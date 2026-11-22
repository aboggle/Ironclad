use crate::search::Searcher;
use cozy_chess::{Board, Color};
use std::io::{self, BufRead, Write};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
pub fn run() {
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
                        if let Ok(mv) = crate::chess::parse_uci_move(&board, s) {
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
