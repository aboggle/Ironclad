use crate::search::{SearchParams, Searcher, Tt};
use cozy_chess::{Board, Color};
use std::io::{self, BufRead, Write};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Limits {
    depth: usize,
    movetime: Option<u64>,
    wtime: Option<u64>,
    btime: Option<u64>,
    winc: u64,
    binc: u64,
    movestogo: Option<u64>,
    nodes: u64,
    infinite: bool,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            depth: 120,
            movetime: None,
            wtime: None,
            btime: None,
            winc: 0,
            binc: 0,
            movestogo: None,
            nodes: u64::MAX,
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
            "nodes" => {
                if let Some(v) = val {
                    l.nodes = v.max(1);
                    i += 1;
                }
            }
            "infinite" => l.infinite = true,
            _ => {}
        }
        i += 1;
    }
    l
}

/// Time budget for a move, in milliseconds. Splits the clock across the
/// expected remaining moves, adds most of the increment, keeps a safety
/// margin for overhead, and falls back to nearly all remaining time in an
/// emergency (when the clock is almost gone).
fn time_budget(l: &Limits, side: Color, overhead: u64) -> Option<u64> {
    if let Some(ms) = l.movetime {
        return Some(ms.saturating_sub(overhead).max(1));
    }
    if l.infinite {
        return None;
    }
    let (time, inc) = if side == Color::White {
        (l.wtime?, l.winc)
    } else {
        (l.btime?, l.binc)
    };
    let usable = time.saturating_sub(overhead);
    if usable < 50 {
        // Emergency: burn most of what is left rather than flag.
        return Some(time.saturating_sub(5).max(1));
    }
    let mtg = l.movestogo.unwrap_or(20).max(1);
    let base = usable / mtg + inc * 3 / 4;
    Some(base.min(usable * 6 / 10).max(1))
}

struct SearchHandle {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}
fn stop_search(handle: &mut Option<SearchHandle>) {
    if let Some(h) = handle.take() {
        h.stop.store(true, Ordering::Relaxed);
        let _ = h.handle.join();
    }
}

const OPTIONS: &str = "id name Ironclad 0.1.0\nid author OpenAI\n\
    option name Hash type spin default 64 min 1 max 2048\n\
    option name Move Overhead type spin default 20 min 0 max 500\n\
    uciok";

pub fn run() {
    let stdin = io::stdin();
    let mut board = Board::default();
    let mut hash_mb = 64usize;
    let mut overhead = 20u64;
    // Persistent hash table: survives across searches, cleared on ucinewgame.
    let mut tt: Option<Arc<Tt>> = None;
    // Hashes of the positions played before the root (repetition detection).
    let mut history: Vec<u64> = Vec::new();
    let mut search: Option<SearchHandle> = None;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts[0] {
            "uci" => println!("{OPTIONS}"),
            "isready" => println!("readyok"),
            "setoption" => {
                let name_pos = parts.iter().position(|&x| x == "name");
                let value_pos = parts.iter().position(|&x| x == "value");
                if let (Some(n), Some(v)) = (name_pos, value_pos) {
                    if n + 1 >= v || v + 1 >= parts.len() {
                        continue;
                    }
                    let name = parts[n + 1..v].join(" ").to_ascii_lowercase();
                    let raw = parts[v + 1];
                    if let Ok(value) = raw.parse::<i32>() {
                        match name.as_str() {
                            "hash" => {
                                let mb = value.clamp(1, 2048) as usize;
                                if mb != hash_mb {
                                    hash_mb = mb;
                                    tt = None; // resized on the next search
                                }
                            }
                            "move overhead" => overhead = value.clamp(0, 500) as u64,
                            _ => {}
                        }
                    }
                }
            }
            "ucinewgame" => {
                stop_search(&mut search);
                board = Board::default();
                history.clear();
                // Fresh game, fresh hash table (rebuilt on the next search).
                tt = None;
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
                history.clear();
                history.push(board.hash());
                if let Some(m) = moves_pos {
                    for s in &parts[m + 1..] {
                        if let Ok(mv) = crate::chess::parse_uci_move(&board, s) {
                            if board.try_play(mv).is_err() {
                                eprintln!("info string illegal move {s}");
                                break;
                            }
                            history.push(board.hash());
                        } else {
                            break;
                        }
                    }
                }
                // The root position itself must not count as its own history.
                history.pop();
            }
            "go" if parts.get(1) == Some(&"perft") => {
                // Common UCI extension: node counts for the current position.
                if let Some(depth) = parts.get(2).and_then(|s| s.parse::<u32>().ok()) {
                    let start = Instant::now();
                    let nodes = crate::chess::perft(&board, depth);
                    println!(
                        "info string perft nodes {} time {}",
                        nodes,
                        start.elapsed().as_millis()
                    );
                }
            }
            "go" => {
                stop_search(&mut search);
                let l = parse_go(&parts);
                let side = board.side_to_move();
                let budget = time_budget(&l, side, overhead);
                let deadline = budget.map(|ms| Instant::now() + Duration::from_millis(ms));
                let flag = Arc::new(AtomicBool::new(false));
                let dl = Arc::new(Mutex::new(deadline));
                let tt_arc = tt.get_or_insert_with(|| Arc::new(Tt::new(hash_mb))).clone();
                let params = SearchParams {
                    tt: tt_arc.clone(),
                    stop: flag.clone(),
                    deadline: dl.clone(),
                    root_history: history.clone(),
                    max_nodes: l.nodes,
                };
                let b = board.clone();
                let depth = l.depth;
                let handle = thread::spawn(move || {
                    let mut s = Searcher::new(params);
                    s.run(b, depth);
                });
                search = Some(SearchHandle { stop: flag, handle });
            }
            "stop" => stop_search(&mut search),
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
