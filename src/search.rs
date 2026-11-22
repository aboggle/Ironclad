use cozy_chess::{Board, Move, Piece};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

const INF: i32 = 32_000;
const MATE: i32 = 30_000;
const MAX_PLY: usize = 128;
const VALUES: [i32; 6] = [100, 320, 335, 500, 950, 0];

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

pub struct Searcher {
    tt: Vec<Option<Entry>>,
    nodes: u64,
    stop: Arc<AtomicBool>,
    started: Instant,
    deadline: Option<Instant>,
    killers: [[Option<Move>; 2]; MAX_PLY],
    history: [[[i32; 64]; 64]; 2],

    root_best: Option<Move>,
    seldepth: usize,
}

impl Searcher {
    pub fn new(hash_mb: usize, stop: Arc<AtomicBool>, deadline: Option<Instant>) -> Self {
        let count = (hash_mb.max(1) * 1024 * 1024 / std::mem::size_of::<Option<Entry>>()).max(1);
        Self {
            tt: vec![None; count],
            nodes: 0,
            stop,
            started: Instant::now(),
            deadline,
            killers: [[None; 2]; MAX_PLY],
            history: [[[0; 64]; 64]; 2],
            root_best: None,
            seldepth: 0,
        }
    }
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || self.deadline.is_some_and(|d| Instant::now() >= d)
    }
    fn poll(&mut self) -> bool {
        if self.nodes & 2047 == 0 && self.stopped() {
            self.stop.store(true, Ordering::Relaxed);
        }
        self.stop.load(Ordering::Relaxed)
    }
    fn generate(board: &Board) -> Vec<Move> {
        let mut out = Vec::with_capacity(64);
        board.generate_moves(|group| {
            out.extend(group);
            false
        });
        out
    }
    fn capture_value(board: &Board, mv: Move) -> i32 {
        let victim = if crate::chess::is_en_passant(board, mv) {
            VALUES[Piece::Pawn as usize]
        } else {
            board.piece_on(mv.to).map_or(0, |p| VALUES[p as usize])
        };
        let attacker = board.piece_on(mv.from).map_or(0, |p| VALUES[p as usize]);
        victim * 16 - attacker
    }
    fn order(&self, board: &Board, moves: &mut [Move], hash: Option<Move>, ply: usize) {
        let side = board.side_to_move() as usize;
        moves.sort_unstable_by_key(|&m| {
            if Some(m) == hash {
                return 2_000_000;
            }
            if crate::chess::is_capture(board, m) {
                return 1_000_000 + Self::capture_value(board, m);
            }
            if m.promotion.is_some() {
                return 900_000;
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
        &mut self,
        key: u64,
        best: Option<Move>,
        score: i32,
        depth: i32,
        bound: Bound,
        ply: usize,
    ) {
        let idx = key as usize % self.tt.len();
        let score = if score > MATE - 1000 {
            score + ply as i32
        } else if score < -MATE + 1000 {
            score - ply as i32
        } else {
            score
        };
        if self.tt[idx].is_none_or(|e| e.key == key || depth >= e.depth as i32) {
            self.tt[idx] = Some(Entry {
                key,
                best,
                score,
                depth: depth as i16,
                bound,
            });
        }
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
    fn quiescence(&mut self, board: &Board, mut alpha: i32, beta: i32, ply: usize) -> i32 {
        self.nodes += 1;
        self.seldepth = self.seldepth.max(ply);
        if self.poll() {
            return crate::eval::evaluate(board);
        }
        if ply >= MAX_PLY - 1 {
            return crate::eval::evaluate(board);
        }
        let in_check = !board.checkers().is_empty();
        let stand = crate::eval::evaluate(board);
        if !in_check {
            if stand >= beta {
                return beta;
            }
            alpha = alpha.max(stand);
        }
        let mut moves = Self::generate(board);
        if moves.is_empty() {
            return if in_check { -MATE + ply as i32 } else { 0 };
        }
        self.order(board, &mut moves, None, ply);
        for mv in moves {
            if !in_check && !crate::chess::is_capture(board, mv) && mv.promotion.is_none() {
                continue;
            }
            let mut child = board.clone();
            child.play_unchecked(mv);
            let score = -self.quiescence(&child, -beta, -alpha, ply + 1);
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
    fn search(
        &mut self,
        board: &Board,
        depth: i32,
        mut alpha: i32,
        beta: i32,
        ply: usize,
        pv: bool,
    ) -> i32 {
        if depth <= 0 {
            return self.quiescence(board, alpha, beta, ply);
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
        let original_alpha = alpha;
        let idx = key as usize % self.tt.len();
        let hit = self.tt[idx].filter(|e| e.key == key);
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
        // Null-move pruning is deliberately omitted: Board does not expose a safe null move.
        let mut moves = Self::generate(board);
        if moves.is_empty() {
            return if in_check { -MATE + ply as i32 } else { 0 };
        }
        self.order(board, &mut moves, hit.and_then(|e| e.best), ply);
        let mut best_score = -INF;
        let mut best_move = None;
        let mut legal = 0;
        for (i, mv) in moves.iter().copied().enumerate() {
            let capture = crate::chess::is_capture(board, mv);
            let pawn_move = board.piece_on(mv.from) == Some(Piece::Pawn);
            let mut child = board.clone();
            child.play_unchecked(mv);
            legal += 1;
            let gives_check = !child.checkers().is_empty();
            let mut next_depth = depth - 1;
            if gives_check && depth >= 2 && ply < 8 {
                next_depth += 1;
            }
            let score = if i == 0 {
                -self.search(&child, next_depth, -beta, -alpha, ply + 1, pv)
            } else {
                let reduction = if depth >= 3
                    && i >= 4
                    && !capture
                    && !pawn_move
                    && !in_check
                    && !gives_check
                {
                    1 + (i >= 10 && depth >= 6) as i32
                } else {
                    0
                };
                let mut s = -self.search(
                    &child,
                    (next_depth - reduction).max(0),
                    -alpha - 1,
                    -alpha,
                    ply + 1,
                    false,
                );
                if s > alpha && reduction > 0 {
                    s = -self.search(&child, next_depth, -alpha - 1, -alpha, ply + 1, false);
                }
                if s > alpha && s < beta {
                    s = -self.search(&child, next_depth, -beta, -alpha, ply + 1, pv);
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
                    if self.killers[ply][0] != Some(mv) {
                        self.killers[ply][1] = self.killers[ply][0];
                        self.killers[ply][0] = Some(mv);
                    }
                    let h = &mut self.history[board.side_to_move() as usize][mv.from as usize]
                        [mv.to as usize];
                    *h = (*h + depth * depth).min(20_000);
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
            if self.stopped() {
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
                score = self.search(&board, depth as i32, alpha, beta, 0, true);
                if self.stop.load(Ordering::Relaxed) {
                    break;
                }
                if score <= alpha && alpha > -INF {
                    alpha = -INF;
                    continue;
                }
                if score >= beta && beta < INF {
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
                self.tt.iter().take(1000).filter(|e| e.is_some()).count() / 10,
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
            let Some(e) = self.tt[key as usize % self.tt.len()].filter(|e| e.key == key) else {
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
