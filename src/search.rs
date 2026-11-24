use crate::chess::{is_capture, is_castling, is_en_passant};
use cozy_chess::{Board, Move, Piece};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const INF: i32 = 32_000;
const MATE: i32 = 30_000;
const MAX_PLY: usize = 128;
const VALUES: [i32; 6] = [100, 320, 335, 500, 950, 0];
const HISTORY_LIMIT: i32 = 20_000;
/// Number of independently locked hash-table shards.
const TT_SHARDS: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bound {
    Exact,
    Lower,
    Upper,
}
#[derive(Clone, Copy)]
struct Entry {
    key: u64,
    best: Option<Move>,
    score: i32,
    depth: i16,
    bound: Bound,
}

struct Shard {
    entries: Mutex<Vec<Option<Entry>>>,
}

/// Transposition table reused between searches.
///
/// Sharded with `try_lock`: under contention a probe or store simply fails
/// (a miss or a skipped write) instead of blocking the search.
pub struct Tt {
    shards: Vec<Shard>,
    per_shard: usize,
}
impl Tt {
    pub fn new(hash_mb: usize) -> Self {
        let count =
            (hash_mb.max(1) * 1024 * 1024 / std::mem::size_of::<Option<Entry>>()).max(TT_SHARDS);
        let per_shard = (count / TT_SHARDS).max(1);
        Self {
            shards: (0..TT_SHARDS)
                .map(|_| Shard {
                    entries: Mutex::new(vec![None; per_shard]),
                })
                .collect(),
            per_shard,
        }
    }
    fn locate(&self, key: u64) -> (usize, usize) {
        let n = self.shards.len();
        let shard = key as usize % n;
        let idx = ((key / n as u64) as usize) % self.per_shard;
        (shard, idx)
    }
    fn probe(&self, key: u64) -> Option<Entry> {
        let (s, i) = self.locate(key);
        let entries = self.shards[s].entries.try_lock().ok()?;
        let entry = entries[i];
        drop(entries);
        entry.filter(|e| e.key == key)
    }
    fn store(&self, entry: Entry) {
        let (s, i) = self.locate(entry.key);
        let Ok(mut entries) = self.shards[s].entries.try_lock() else {
            return;
        };
        let replace = entries[i].is_none_or(|e| e.key == entry.key || entry.depth >= e.depth);
        if replace {
            entries[i] = Some(entry);
        }
    }
    /// Occupancy in permille, sampled from the first entries of each shard.
    fn hashfull(&self) -> usize {
        let mut used = 0;
        let mut sampled = 0;
        for shard in &self.shards {
            let Ok(entries) = shard.entries.try_lock() else {
                continue;
            };
            for entry in entries.iter().take(64) {
                if entry.is_some() {
                    used += 1;
                }
                sampled += 1;
                if sampled >= 1000 {
                    break;
                }
            }
            if sampled >= 1000 {
                break;
            }
        }
        used * 1000 / sampled.max(1)
    }
}

/// Square-pair index (0..4096) for counter-move heuristics.
fn move_index(mv: Move) -> usize {
    mv.from as usize * 64 + mv.to as usize
}

/// Shared state passed to the asynchronous search.
#[derive(Clone)]
pub struct SearchParams {
    pub tt: Arc<Tt>,
    pub stop: Arc<AtomicBool>,
    pub deadline: Arc<Mutex<Option<Instant>>>,
    /// Hashes of the positions before the root, for repetition detection.
    pub root_history: Vec<u64>,
    /// Hard node budget for the search; `u64::MAX` means unlimited.
    pub max_nodes: u64,
}

pub struct Searcher {
    tt: Arc<Tt>,
    nodes: u64,
    stop: Arc<AtomicBool>,
    deadline: Arc<Mutex<Option<Instant>>>,
    max_nodes: u64,
    started: Instant,
    killers: [[Option<Move>; 2]; MAX_PLY],
    history: [[[i32; 64]; 64]; 2],
    counters: [[Option<Move>; 4096]; 2],

    root_best: Option<Move>,
    seldepth: usize,
    root_history: Vec<u64>,
    /// Ancestor hashes of the current search path (repetition detection).
    hash_path: [u64; MAX_PLY + 2],
}

impl Searcher {
    pub fn new(p: SearchParams) -> Self {
        Self {
            tt: p.tt,
            nodes: 0,
            stop: p.stop,
            deadline: p.deadline,
            max_nodes: p.max_nodes,
            started: Instant::now(),
            killers: [[None; 2]; MAX_PLY],
            history: [[[0; 64]; 64]; 2],
            counters: [[None; 4096]; 2],
            root_best: None,
            seldepth: 0,
            root_history: p.root_history,
            hash_path: [0; MAX_PLY + 2],
        }
    }
    fn stopped(&self) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return true;
        }
        self.deadline
            .lock()
            .map(|d| d.is_some_and(|t| Instant::now() >= t))
            .unwrap_or(false)
    }
    fn poll(&mut self) -> bool {
        if self.nodes > self.max_nodes {
            self.stop.store(true, Ordering::Relaxed);
        }
        if self.nodes & 2047 == 0 && self.stopped() {
            self.stop.store(true, Ordering::Relaxed);
        }
        self.stop.load(Ordering::Relaxed)
    }
    /// 50-move rule and repetition within the irreversible-move window.
    fn is_draw(&self, board: &Board, key: u64, ply: usize, fictional: bool) -> bool {
        if ply == 0 || fictional {
            return false;
        }
        if board.halfmove_clock() >= 100 {
            return true;
        }
        let clock = board.halfmove_clock() as usize;
        if clock < 2 {
            return false;
        }
        let window = clock.min(ply);
        for &h in &self.hash_path[ply - window..ply] {
            if h == key {
                return true;
            }
        }
        let before_root = clock.saturating_sub(ply);
        for &h in self.root_history.iter().rev().take(before_root) {
            if h == key {
                return true;
            }
        }
        false
    }
    fn draw_score(&self, board: &Board, key: u64, ply: usize, fictional: bool) -> Option<i32> {
        if !self.is_draw(board, key, ply, fictional) {
            return None;
        }
        // Checkmate takes precedence over the fifty-move and repetition draws.
        if !board.checkers().is_empty() && !board.generate_moves(|_| true) {
            Some(-MATE + ply as i32)
        } else {
            Some(0)
        }
    }
    fn generate(board: &Board) -> Vec<Move> {
        let mut out = Vec::with_capacity(64);
        board.generate_moves(|group| {
            out.extend(group);
            false
        });
        out
    }
    fn has_non_pawn_material(board: &Board) -> bool {
        let own = board.colors(board.side_to_move());
        let majors = board.pieces(Piece::Rook) | board.pieces(Piece::Queen);
        if !(majors & own).is_empty() {
            return true;
        }
        let minors = (board.pieces(Piece::Knight) | board.pieces(Piece::Bishop)) & own;
        minors.len() >= 2
    }
    fn capture_value(board: &Board, mv: Move) -> i32 {
        let victim = if is_en_passant(board, mv) {
            VALUES[Piece::Pawn as usize]
        } else {
            board.piece_on(mv.to).map_or(0, |p| VALUES[p as usize])
        };
        let attacker = board.piece_on(mv.from).map_or(0, |p| VALUES[p as usize]);
        victim * 16 - attacker
    }
    /// Material gain from a capture sequence, assuming each side uses its
    /// least valuable legal recapture. Legal move generation handles pins,
    /// king captures, and x-rays as pieces leave the target square.
    fn static_exchange(board: &Board, mv: Move) -> i32 {
        if !is_capture(board, mv) {
            return mv.promotion.map_or(0, |piece| {
                VALUES[piece as usize] - VALUES[Piece::Pawn as usize]
            });
        }

        let victim = if is_en_passant(board, mv) {
            VALUES[Piece::Pawn as usize]
        } else {
            board
                .piece_on(mv.to)
                .map_or(0, |piece| VALUES[piece as usize])
        };
        let mut gains = [0; 32];
        gains[0] = victim
            + mv.promotion.map_or(0, |piece| {
                VALUES[piece as usize] - VALUES[Piece::Pawn as usize]
            });

        let mut position = board.clone();
        let mut occupant = mv
            .promotion
            .or_else(|| board.piece_on(mv.from))
            .unwrap_or(Piece::Pawn);
        position.play_unchecked(mv);
        let mut depth = 0;

        while depth + 1 < gains.len() {
            let Some(reply) = Self::least_attacker(&position, mv.to) else {
                break;
            };

            depth += 1;
            gains[depth] = VALUES[occupant as usize] - gains[depth - 1];
            let attacker = position.piece_on(reply.from).unwrap_or(Piece::Pawn);
            if let Some(promotion) = reply.promotion {
                gains[depth] += VALUES[promotion as usize] - VALUES[Piece::Pawn as usize];
                occupant = promotion;
            } else {
                occupant = attacker;
            }
            position.play_unchecked(reply);
        }

        while depth > 0 {
            gains[depth - 1] = -(-gains[depth - 1]).max(gains[depth]);
            depth -= 1;
        }
        gains[0]
    }
    fn least_attacker(board: &Board, target: cozy_chess::Square) -> Option<Move> {
        for piece in [
            Piece::Pawn,
            Piece::Knight,
            Piece::Bishop,
            Piece::Rook,
            Piece::Queen,
            Piece::King,
        ] {
            for from in board.colors(board.side_to_move()) {
                if board.piece_on(from) != Some(piece)
                    || !attacks_square(board, from, target, piece, board.side_to_move())
                {
                    continue;
                }
                let promotion = (piece == Piece::Pawn
                    && target.relative_to(board.side_to_move()).rank() as usize == 7)
                    .then_some(Piece::Queen);
                let mv = Move {
                    from,
                    to: target,
                    promotion,
                };
                if board.is_legal(mv) {
                    return Some(mv);
                }
            }
        }
        None
    }
    fn update_history(&mut self, side: usize, mv: Move, bonus: i32) {
        let bonus = bonus.clamp(-HISTORY_LIMIT, HISTORY_LIMIT);
        let value = &mut self.history[side][mv.from as usize][mv.to as usize];
        *value += bonus - *value * bonus.abs() / HISTORY_LIMIT;
        *value = (*value).clamp(-HISTORY_LIMIT, HISTORY_LIMIT);
    }
    fn order(&self, board: &Board, moves: &mut [Move], hash: Option<Move>, ply: usize) {
        let side = board.side_to_move() as usize;
        moves.sort_unstable_by_key(|&m| {
            if Some(m) == hash {
                return 2_000_000;
            }
            if is_capture(board, m) {
                let see = Self::static_exchange(board, m);
                let good_capture_bonus = if see >= 0 { 1_000_000 } else { 100_000 };
                return good_capture_bonus + see * 12 + Self::capture_value(board, m);
            }
            if let Some(promotion) = m.promotion {
                return 900_000 + Self::static_exchange(board, m) + VALUES[promotion as usize];
            }
            // Castling is quiet but worth finding early.
            if is_castling(board, m) {
                return 830_000;
            }
            if ply < MAX_PLY && self.counters[side][move_index(m)] == Some(m) {
                return 720_000;
            }
            if ply < MAX_PLY && self.killers[ply][0] == Some(m) {
                return 800_000;
            }
            if ply < MAX_PLY && self.killers[ply][1] == Some(m) {
                return 790_000;
            }
            self.history[side][m.from as usize][m.to as usize]
        });
        moves.reverse();
    }
    fn store(
        &self,
        key: u64,
        best: Option<Move>,
        score: i32,
        depth: i32,
        bound: Bound,
        ply: usize,
    ) {
        let score = if score > MATE - 1000 {
            score + ply as i32
        } else if score < -MATE + 1000 {
            score - ply as i32
        } else {
            score
        };
        self.tt.store(Entry {
            key,
            best,
            score,
            depth: depth as i16,
            bound,
        });
    }
    fn tt_score(score: i32, ply: usize) -> i32 {
        if score > MATE - 1000 {
            score - ply as i32
        } else if score < -MATE + 1000 {
            score + ply as i32
        } else {
            score
        }
    }
    fn quiescence(
        &mut self,
        board: &Board,
        mut alpha: i32,
        beta: i32,
        ply: usize,
        fictional: bool,
    ) -> i32 {
        self.nodes += 1;
        self.seldepth = self.seldepth.max(ply);
        if self.poll() {
            return crate::eval::evaluate(board);
        }
        if ply >= MAX_PLY - 1 {
            return crate::eval::evaluate(board);
        }
        let key = board.hash();
        if let Some(score) = self.draw_score(board, key, ply, fictional) {
            return score;
        }
        if !fictional {
            self.hash_path[ply] = key;
        }
        let in_check = !board.checkers().is_empty();
        let stand = crate::eval::evaluate(board);
        if !in_check {
            if stand >= beta {
                // Stand-pat is invalid in stalemate. Check for a legal move
                // before accepting the cutoff; the ordinary move loop below
                // handles stalemates when stand-pat does not cut off.
                return if board.generate_moves(|_| true) {
                    beta
                } else {
                    0
                };
            }
            alpha = alpha.max(stand);
        }
        let mut moves = Self::generate(board);
        if moves.is_empty() {
            return if in_check { -MATE + ply as i32 } else { 0 };
        }
        let tt_move = self.tt.probe(key).and_then(|e| e.best);
        self.order(board, &mut moves, tt_move, ply);
        for mv in moves {
            if !in_check && !is_capture(board, mv) && mv.promotion.is_none() {
                continue;
            }
            let mut child = board.clone();
            child.play_unchecked(mv);
            // Delta pruning: skip quiet captures that can't realistically
            // bring us above alpha — the SEE value already includes any
            // recapture margin.
            if !in_check
                && mv.promotion.is_none()
                && child.checkers().is_empty()
                && stand + Self::static_exchange(board, mv) < alpha
            {
                continue;
            }
            let score = -self.quiescence(&child, -beta, -alpha, ply + 1, fictional);
            if self.stop.load(Ordering::Relaxed) {
                return alpha;
            }
            if score >= beta {
                return beta;
            }
            alpha = alpha.max(score);
        }
        alpha
    }
    #[allow(clippy::too_many_arguments)]
    fn search(
        &mut self,
        board: &Board,
        depth: i32,
        mut alpha: i32,
        beta: i32,
        ply: usize,
        pv: bool,
        fictional: bool,
        excluded: Option<Move>,
    ) -> i32 {
        if depth <= 0 {
            return self.quiescence(board, alpha, beta, ply, fictional);
        }
        self.nodes += 1;
        self.seldepth = self.seldepth.max(ply);
        if self.poll() {
            return crate::eval::evaluate(board);
        }
        if ply >= MAX_PLY - 1 {
            return crate::eval::evaluate(board);
        }
        let key = board.hash();

        if let Some(score) = self.draw_score(board, key, ply, fictional) {
            return score;
        }
        if !fictional {
            self.hash_path[ply] = key;
        }

        let original_alpha = alpha;
        let hit = self.tt.probe(key);
        if let Some(e) = hit {
            if !pv && e.depth as i32 >= depth {
                let score = Self::tt_score(e.score, ply);
                match e.bound {
                    Bound::Exact => return score,
                    Bound::Lower if score >= beta => return score,
                    Bound::Upper if score <= alpha => return score,
                    _ => {}
                }
            }
        }
        let in_check = !board.checkers().is_empty();

        // Null-move pruning: pass when the position is quiet and we have
        // non-pawn material (zugzwang-prone endgames are excluded). Deeper
        // cutoffs are verified by a normal re-search.
        if ply > 0
            && depth >= 3
            && !in_check
            && beta > -MATE + 1000
            && beta < MATE - 1000
            && Self::has_non_pawn_material(board)
        {
            if let Some(null) = board.null_move() {
                let reduction = 2 + depth / 6;
                let score = -self.search(
                    &null,
                    (depth - 1 - reduction).max(0),
                    -beta,
                    -beta + 1,
                    ply + 1,
                    false,
                    true,
                    None,
                );
                if self.stop.load(Ordering::Relaxed) {
                    return alpha;
                }
                if score >= beta {
                    if depth < 7 {
                        return if board.generate_moves(|_| true) {
                            beta
                        } else {
                            0
                        };
                    }
                    let verify =
                        self.search(board, depth - 1, beta - 1, beta, ply, pv, fictional, None);
                    if verify >= beta {
                        return verify;
                    }
                }
            }
        }

        // Reverse futility (static null move) and razoring on quiet,
        // non-PV nodes. Pruning is disabled at the root so every root search
        // still produces a move.
        if ply > 0 && !in_check && !pv && depth <= 6 {
            let static_eval = crate::eval::evaluate(board);
            if beta < MATE - 1000 && static_eval - (80 + 100 * depth) >= beta {
                return if board.generate_moves(|_| true) {
                    beta
                } else {
                    0
                };
            }
            if depth <= 2 && static_eval + 300 * depth < alpha {
                return self.quiescence(board, alpha, beta, ply, fictional);
            }
        }

        let mut moves = Self::generate(board);
        if moves.is_empty() {
            return if in_check { -MATE + ply as i32 } else { 0 };
        }
        self.order(board, &mut moves, hit.and_then(|e| e.best), ply);
        let mut best_score = -INF;
        let mut best_move = None;
        let mut legal = 0;
        let history_side = board.side_to_move() as usize;
        for (i, mv) in moves.iter().copied().enumerate() {
            if excluded == Some(mv) {
                continue;
            }
            let capture = is_capture(board, mv);
            let pawn_move = board.piece_on(mv.from) == Some(Piece::Pawn);
            let castling = is_castling(board, mv);
            let mut child = board.clone();
            child.play_unchecked(mv);
            legal += 1;
            let gives_check = !child.checkers().is_empty();
            let mut next_depth = depth - 1;
            if gives_check && depth >= 2 && ply < 8 {
                next_depth += 1;
            }
            let score = if i == 0 {
                // Singular extension: if no alternative reaches alpha on its
                // own, the PV move is singular and deserves an extra depth.
                if pv && depth >= 10 && !capture && !gives_check {
                    let probe = self.search(
                        &child,
                        depth - 2,
                        alpha,
                        alpha + 1,
                        ply + 1,
                        false,
                        fictional,
                        Some(mv),
                    );
                    if probe < alpha + 1 {
                        next_depth += 1;
                    }
                }
                -self.search(
                    &child,
                    next_depth,
                    -beta,
                    -alpha,
                    ply + 1,
                    pv,
                    fictional,
                    None,
                )
            } else {
                // Lower the reduction on moves whose history is promising.
                let mut red = 0i32;
                if depth >= 3 {
                    if i >= 4 && !capture && !pawn_move && !castling && !in_check && !gives_check {
                        red = 1 + (i >= 10 && depth >= 6) as i32;
                        if self.history[history_side][mv.from as usize][mv.to as usize]
                            < -HISTORY_LIMIT / 4
                        {
                            red += 1;
                        }
                    } else if is_capture(board, mv) && Self::static_exchange(board, mv) < 0 {
                        red = 1;
                    }
                }
                let mut s = -self.search(
                    &child,
                    (next_depth - red).max(0),
                    -alpha - 1,
                    -alpha,
                    ply + 1,
                    false,
                    fictional,
                    None,
                );
                if s > alpha && red > 0 {
                    s = -self.search(
                        &child,
                        next_depth,
                        -alpha - 1,
                        -alpha,
                        ply + 1,
                        false,
                        fictional,
                        None,
                    );
                }
                if s > alpha && s < beta {
                    s = -self.search(
                        &child,
                        next_depth,
                        -beta,
                        -alpha,
                        ply + 1,
                        pv,
                        fictional,
                        None,
                    );
                }
                s
            };
            if self.stop.load(Ordering::Relaxed) {
                return alpha;
            }
            if score > best_score {
                best_score = score;
                best_move = Some(mv);
            }
            if score > alpha {
                alpha = score;
                if ply == 0 {
                    self.root_best = Some(mv);
                }
            }
            if alpha >= beta {
                if !capture && ply < MAX_PLY {
                    self.counters[history_side][move_index(mv)] = Some(mv);
                    if self.killers[ply][0] != Some(mv) {
                        self.killers[ply][1] = self.killers[ply][0];
                        self.killers[ply][0] = Some(mv);
                    }
                    let bonus = (depth * depth * 16).min(HISTORY_LIMIT);
                    self.update_history(history_side, mv, bonus);
                    for &failed in &moves[..i] {
                        if !is_capture(board, failed) {
                            self.update_history(history_side, failed, -bonus / 2);
                        }
                    }
                }
                break;
            }
        }
        if legal == 0 {
            return if in_check { -MATE + ply as i32 } else { 0 };
        }
        let bound = if best_score <= original_alpha {
            Bound::Upper
        } else if best_score >= beta {
            Bound::Lower
        } else {
            Bound::Exact
        };
        self.store(key, best_move, best_score, depth, bound, ply);
        best_score
    }
    pub fn run(&mut self, board: Board, max_depth: usize) {
        let mut best = Self::generate(&board).first().copied();
        let mut prev = 0;
        for depth in 1..=max_depth.min(120) {
            if self.stopped() || self.nodes > self.max_nodes {
                break;
            }
            self.seldepth = 0;
            let window = if depth >= 4 { 35 } else { INF };
            let (mut alpha, mut beta) = if window == INF {
                (-INF, INF)
            } else {
                (prev - window, prev + window)
            };
            let mut score;
            loop {
                self.root_best = best;
                score = self.search(&board, depth as i32, alpha, beta, 0, true, false, None);
                if self.stop.load(Ordering::Relaxed) {
                    break;
                }
                if score <= alpha && alpha > -INF {
                    eprintln!(
                        "info string aspiration depth {} score {}: lowering window",
                        depth,
                        score_uci(score)
                    );
                    alpha = -INF;
                    continue;
                }
                if score >= beta && beta < INF {
                    eprintln!(
                        "info string aspiration depth {} score {}: raising window",
                        depth,
                        score_uci(score)
                    );
                    beta = INF;
                    continue;
                }
                break;
            }
            if self.stop.load(Ordering::Relaxed) {
                break;
            }
            best = self.root_best.or(best);
            prev = score;
            let elapsed = self.started.elapsed().as_millis().max(1);
            let pv = self.extract_pv(&board, depth);
            println!(
                "info depth {} seldepth {} score {} nodes {} nps {} hashfull {} time {} pv {}",
                depth,
                self.seldepth,
                score_uci(score),
                self.nodes,
                self.nodes as u128 * 1000 / elapsed,
                self.tt.hashfull(),
                elapsed,
                crate::chess::format_pv(&board, &pv)
            );
            let _ = io::stdout().flush();
            if score.abs() > MATE - 1000 {
                break;
            }
        }
        println!(
            "bestmove {}",
            best.map_or_else(
                || "0000".to_string(),
                |m| crate::chess::format_uci_move(&board, m)
            )
        );
        let _ = io::stdout().flush();
    }
    fn extract_pv(&self, board: &Board, depth: usize) -> Vec<Move> {
        let mut b = board.clone();
        let mut pv = Vec::new();
        let mut seen = Vec::new();
        for _ in 0..depth.min(24) {
            let key = b.hash();
            if seen.contains(&key) {
                break;
            }
            seen.push(key);
            let Some(e) = self.tt.probe(key).filter(|e| e.key == key) else {
                break;
            };
            let Some(mv) = e.best else {
                break;
            };
            if !b.is_legal(mv) {
                break;
            }
            pv.push(mv);
            b.play_unchecked(mv);
        }
        pv
    }
}

fn attacks_square(
    board: &Board,
    from: cozy_chess::Square,
    to: cozy_chess::Square,
    piece: Piece,
    color: cozy_chess::Color,
) -> bool {
    let df = to.file() as i8 - from.file() as i8;
    let dr = to.rank() as i8 - from.rank() as i8;
    let diagonal = df != 0 && df.abs() == dr.abs();
    let straight = (df == 0) != (dr == 0);
    let attacks = match piece {
        Piece::Pawn => {
            dr == if color == cozy_chess::Color::White {
                1
            } else {
                -1
            } && df.abs() == 1
        }
        Piece::Knight => (df.abs() == 1 && dr.abs() == 2) || (df.abs() == 2 && dr.abs() == 1),
        Piece::King => df.abs() <= 1 && dr.abs() <= 1 && (df != 0 || dr != 0),
        Piece::Bishop => diagonal,
        Piece::Rook => straight,
        Piece::Queen => diagonal || straight,
    };
    if !attacks {
        return false;
    }
    if !matches!(piece, Piece::Bishop | Piece::Rook | Piece::Queen) {
        return true;
    }
    let step_file = df.signum();
    let step_rank = dr.signum();
    let mut square = from;
    while let Some(next) = square.try_offset(step_file, step_rank) {
        if next == to {
            return true;
        }
        if board.occupied() & next.bitboard() != cozy_chess::BitBoard::EMPTY {
            return false;
        }
        square = next;
    }
    false
}

fn score_uci(score: i32) -> String {
    if score.abs() > MATE - 1000 {
        format!(
            "mate {}",
            if score > 0 {
                (MATE - score + 1) / 2
            } else {
                -(MATE + score + 1) / 2
            }
        )
    } else {
        format!("cp {}", score)
    }
}
