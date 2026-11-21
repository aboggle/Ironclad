use cozy_chess::{Board, Color, Move, Piece, Square};
use std::io::{self, BufRead, Write};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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

struct Searcher {
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
    fn new(hash_mb: usize, stop: Arc<AtomicBool>, deadline: Option<Instant>) -> Self {
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
    fn evaluate(board: &Board) -> i32 {
        let side = board.side_to_move();
        let mut score = 0;
        for sq in Square::ALL {
            if let Some(piece) = board.piece_on(sq) {
                let color = board.color_on(sq).unwrap();
                let sign = if color == side { 1 } else { -1 };
                let rel = sq.relative_to(color);
                let rank = rel.rank() as usize;
                let file = rel.file() as usize;
                let center = 7 - ((file.abs_diff(3) + rank.abs_diff(3)).min(6));
                let positional = match piece {
                    Piece::Pawn => rank as i32 * 7 + center as i32 * 2,
                    Piece::Knight => center as i32 * 7,
                    Piece::Bishop => center as i32 * 4,
                    Piece::Rook => rank as i32 * 2,
                    Piece::Queen => center as i32 * 2,
                    Piece::King => {
                        if board.occupied().len() < 14 {
                            center as i32 * 7
                        } else {
                            -(center as i32) * 3
                        }
                    }
                };
                score += sign * (VALUES[piece as usize] + positional);
            }
        }
        score + 12
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
        let victim = board.piece_on(mv.to).map_or(0, |p| VALUES[p as usize]);
        let attacker = board.piece_on(mv.from).map_or(0, |p| VALUES[p as usize]);
        victim * 16 - attacker
    }
    fn order(&self, board: &Board, moves: &mut [Move], hash: Option<Move>, ply: usize) {
        let side = board.side_to_move() as usize;
        moves.sort_unstable_by_key(|&m| {
            if Some(m) == hash {
                return 2_000_000;
            }
            if board.piece_on(m.to).is_some()
                || (board.piece_on(m.from) == Some(Piece::Pawn)
                    && board.en_passant().is_some()
                    && m.to.file() == board.en_passant().unwrap())
            {
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
            return Self::evaluate(board);
        }
        if ply >= MAX_PLY - 1 {
            return Self::evaluate(board);
        }
        let in_check = !board.checkers().is_empty();
        let stand = Self::evaluate(board);
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
            if !in_check && board.piece_on(mv.to).is_none() && mv.promotion.is_none() {
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
            return Self::evaluate(board);
        }
        if ply >= MAX_PLY - 1 {
            return Self::evaluate(board);
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
        let static_eval = Self::evaluate(board);
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
            let capture = board.piece_on(mv.to).is_some();
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
        let _ = static_eval;
        best_score
    }
    fn run(&mut self, board: Board, max_depth: usize) {
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
                pv.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            let _ = io::stdout().flush();
            if score.abs() > MATE - 1000 {
                break;
            }
        }
        println!(
            "bestmove {}",
            best.map_or_else(|| "0000".to_string(), |m| m.to_string())
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

#[derive(Clone)]
struct Limits {
    depth: usize,
    movetime: Option<u64>,
    wtime: Option<u64>,
    btime: Option<u64>,
    winc: u64,
    binc: u64,
    movestogo: Option<u64>,
    infinite: bool,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            depth: 64,
            movetime: None,
            wtime: None,
            btime: None,
            winc: 0,
            binc: 0,
            movestogo: None,
            infinite: false,
        }
    }
}
fn parse_go(words: &[&str]) -> Limits {
    let mut l = Limits::default();
    let mut i = 1;
    while i < words.len() {
        let val = words.get(i + 1).and_then(|s| s.parse::<u64>().ok());
        match words[i] {
            "depth" => {
                if let Some(v) = val {
                    l.depth = v as usize;
                    i += 1;
                }
            }
            "movetime" => {
                if let Some(v) = val {
                    l.movetime = Some(v);
                    i += 1;
                }
            }
            "wtime" => {
                if let Some(v) = val {
                    l.wtime = Some(v);
                    i += 1;
                }
            }
            "btime" => {
                if let Some(v) = val {
                    l.btime = Some(v);
                    i += 1;
                }
            }
            "winc" => {
                if let Some(v) = val {
                    l.winc = v;
                    i += 1;
                }
            }
            "binc" => {
                if let Some(v) = val {
                    l.binc = v;
                    i += 1;
                }
            }
            "movestogo" => {
                if let Some(v) = val {
                    l.movestogo = Some(v);
                    i += 1;
                }
            }
            "infinite" | "ponder" => l.infinite = true,
            _ => {}
        }
        i += 1;
    }
    l
}
fn stop_search(handle: &mut Option<(Arc<AtomicBool>, JoinHandle<()>)>) {
    if let Some((flag, thread)) = handle.take() {
        flag.store(true, Ordering::Relaxed);
        let _ = thread.join();
    }
}
fn main() {
    let stdin = io::stdin();
    let mut board = Board::default();
    let mut hash_mb = 64usize;
    let mut overhead = 20u64;
    let mut search: Option<(Arc<AtomicBool>, JoinHandle<()>)> = None;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts[0] {
            "uci" => {
                println!("id name Ironclad 0.1.0\nid author OpenAI\noption name Hash type spin default 64 min 1 max 2048\noption name Move Overhead type spin default 20 min 0 max 500\nuciok");
            }
            "isready" => println!("readyok"),
            "setoption" => {
                let name_pos = parts.iter().position(|&x| x == "name");
                let value_pos = parts.iter().position(|&x| x == "value");
                if let (Some(n), Some(v)) = (name_pos, value_pos) {
                    let name = parts[n + 1..v].join(" ");
                    if let Ok(value) = parts[v + 1].parse::<usize>() {
                        if name.eq_ignore_ascii_case("Hash") {
                            hash_mb = value.clamp(1, 2048);
                        }
                        if name.eq_ignore_ascii_case("Move Overhead") {
                            overhead = value.min(500) as u64;
                        }
                    }
                }
            }
            "ucinewgame" => {
                stop_search(&mut search);
                board = Board::default();
            }
            "position" => {
                stop_search(&mut search);
                let moves_pos = parts.iter().position(|&x| x == "moves");
                let end = moves_pos.unwrap_or(parts.len());
                if parts.get(1) == Some(&"startpos") {
                    board = Board::default();
                } else if parts.get(1) == Some(&"fen") && parts.len() >= 8 {
                    let fen = parts[2..end].join(" ");
                    if let Ok(b) = Board::from_str(&fen) {
                        board = b;
                    } else {
                        eprintln!("info string invalid FEN");
                    }
                }
                if let Some(m) = moves_pos {
                    for s in &parts[m + 1..] {
                        if let Ok(mv) = Move::from_str(s) {
                            if board.try_play(mv).is_err() {
                                eprintln!("info string illegal move {s}");
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
            }
            "go" => {
                stop_search(&mut search);
                let l = parse_go(&parts);
                let side = board.side_to_move();
                let budget = if let Some(ms) = l.movetime {
                    Some(ms.saturating_sub(overhead).max(1))
                } else if !l.infinite {
                    let (time, inc) = if side == Color::White {
                        (l.wtime, l.winc)
                    } else {
                        (l.btime, l.binc)
                    };
                    time.map(|t| {
                        let mtg = l.movestogo.unwrap_or(30).max(1);
                        (t.saturating_sub(overhead) / mtg + inc * 3 / 4)
                            .min(t.saturating_sub(overhead).max(1))
                            .max(1)
                    })
                } else {
                    None
                };
                let deadline = budget.map(|ms| Instant::now() + Duration::from_millis(ms));
                let flag = Arc::new(AtomicBool::new(false));
                let f = flag.clone();
                let b = board.clone();
                search = Some((
                    flag,
                    thread::spawn(move || {
                        let mut s = Searcher::new(hash_mb, f.clone(), deadline);
                        s.run(b, l.depth);
                    }),
                ));
            }
            "stop" => stop_search(&mut search),
            "ponderhit" => {}
            "quit" => {
                stop_search(&mut search);
                break;
            }
            "debug" | "register" => {}
            _ => {}
        }
        let _ = io::stdout().flush();
    }
    stop_search(&mut search);
}
