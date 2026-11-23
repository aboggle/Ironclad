//! Evaluation with a **necessity-driven** philosophy.
//!
//! Classical chess principles (castle early, hide the king, develop fast) are
//! treated as *emergency responses*, not defaults:
//!
//! - **King danger** is measured concretely (enemy pieces attacking the king
//!   zone, open/semi-open files and enemy majors aimed at it). Castling and
//!   king runs become attractive only when danger demands them.
//! - **Flexibility**: while the king is safe and uncommitted, a small bonus
//!   rewards keeping options open — delayed castling and creative, modern
//!   piece play are preferred when the position is quiet.
//! - **Aggression is symmetric**: pressuring the enemy king raises *their*
//!   danger, so one metric drives both attack and defense.
//! - **Guardrail**: style terms are tiny compared to material, so principles
//!   are broken only when concrete factors justify it.

use cozy_chess::{BitBoard, Board, Color, Piece, Square};

/// Tapered material values (middlegame / endgame).
const MG_VAL: [i32; 6] = [100, 320, 335, 500, 950, 0];
const EG_VAL: [i32; 6] = [114, 310, 330, 520, 930, 0];
/// Phase weights for tapering: N/B = 1, R = 2, Q = 4 (24 per side).
const PHASE_W: [i32; 6] = [0, 1, 1, 2, 4, 0];
const TOTAL_PHASE: i32 = 24;
/// Small bonus for having the move.
const TEMPO: i32 = 12;

/// Mobility points per reachable square (mg / eg).
const MG_MOB: [i32; 6] = [0, 3, 4, 2, 1, 0];
const EG_MOB: [i32; 6] = [0, 2, 3, 2, 2, 0];

/// Weight of an enemy piece that attacks the king zone.
const DANGER_PIECE: [i32; 6] = [6, 12, 12, 16, 28, 0];
/// Extra pressure when the attacker has stepped into the zone itself.
const DANGER_MELEE: i32 = 6;
/// File pressure on the king zone.
const DANGER_OPEN_FILE: i32 = 12;
const DANGER_SEMI_OPEN_FILE: i32 = 8;
const DANGER_ENEMY_MAJOR: i32 = 14;

/// Reward for an uncommitted king while it is safe (mg only).
const FLEXIBILITY: i32 = 10;
/// Danger at which the flexibility reward has fully faded.
const FLEX_FADE: i32 = 24;

const KNIGHT_DELTAS: [(i8, i8); 8] = [
    (1, 2),
    (2, 1),
    (2, -1),
    (1, -2),
    (-1, -2),
    (-2, -1),
    (-2, 1),
    (-1, 2),
];
const KING_DELTAS: [(i8, i8); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];
const BISHOP_DIRS: [(i8, i8); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];
const ROOK_DIRS: [(i8, i8); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const QUEEN_DIRS: [(i8, i8); 8] = [
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
];

/// Side-to-move relative evaluation in centipawns.
pub fn evaluate(board: &Board) -> i32 {
    let occ = board.occupied();
    let mut mg = 0i32;
    let mut eg = 0i32;
    let mut phase = 0i32;
    // Absolute-rank masks per file: pawn pressure and file analysis.
    let mut pawn_mask = [[0u8; 8]; 2];
    let mut major_mask = [[0u8; 8]; 2];
    let mut bishops = [0u8; 2];
    let mut space = [0i32; 2];
    let mut king_sq = [Square::A1; 2];

    // Single pass over all pieces: material, position, mobility, structure.
    for square in Square::ALL {
        let Some(piece) = board.piece_on(square) else {
            continue;
        };
        let Some(color) = board.color_on(square) else {
            continue;
        };
        let ci = usize::from(color == Color::White);
        let sign = if color == Color::White { 1 } else { -1 };
        let relative = square.relative_to(color);
        let rank = relative.rank() as usize;
        let file = relative.file() as usize;
        let abs_rank = square.rank() as usize;
        let abs_file = square.file() as usize;
        let center = 7 - (file.abs_diff(3) + rank.abs_diff(3)).min(6);

        phase += PHASE_W[piece as usize];
        mg += sign * MG_VAL[piece as usize];
        eg += sign * EG_VAL[piece as usize];

        if piece == Piece::Pawn {
            pawn_mask[ci][abs_file] |= 1 << abs_rank;
            if rank >= 3 {
                space[ci] += 1;
            }
        }
        if piece == Piece::Rook || piece == Piece::Queen {
            major_mask[ci][abs_file] |= 1 << abs_rank;
        }
        if piece == Piece::Bishop {
            bishops[ci] += 1;
        }
        if piece == Piece::King {
            king_sq[ci] = square;
        }

        // Piece-square terms (mg / eg). The king is deliberately flat in the
        // middlegame: safety comes from the danger metric, not from a corner
        // bias — keeping the king flexible is a feature, not a bug.
        let (pos_mg, pos_eg) = match piece {
            Piece::Pawn => (rank as i32 * 6 + center as i32 * 2, rank as i32 * 10 + center as i32),
            Piece::Knight => (center as i32 * 7, center as i32 * 5),
            Piece::Bishop => (center as i32 * 4, center as i32 * 5),
            Piece::Rook => (rank as i32 * 2, rank as i32 * 3 + center as i32),
            Piece::Queen => (center as i32 * 2, center as i32 * 3),
            Piece::King => (0, center as i32 * 7),
        };
        mg += sign * pos_mg;
        eg += sign * pos_eg;

        // Mobility: reachable squares for minor pieces and sliders.
        let own = board.colors(color);
        let mob = match piece {
            Piece::Knight => KNIGHT_DELTAS
                .iter()
                .filter(|&&(df, dr)| {
                    square
                        .try_offset(df, dr)
                        .is_some_and(|t| own & t.bitboard() == BitBoard::EMPTY)
                })
                .count() as i32,
            Piece::Bishop => slider_mobility(square, &BISHOP_DIRS, occ, own),
            Piece::Rook => slider_mobility(square, &ROOK_DIRS, occ, own),
            Piece::Queen => slider_mobility(square, &QUEEN_DIRS, occ, own),
            _ => 0,
        };
        mg += sign * mob * MG_MOB[piece as usize];
        eg += sign * mob * EG_MOB[piece as usize];
    }

    // Pawn structure, king safety and style, per color (white-positive).
    let mut white_score = 0i32;
    let mut white_eg = 0i32;
    for color in Color::ALL {
        let ci = usize::from(color == Color::White);
        let sign = if color == Color::White { 1 } else { -1 };

        // Doubled and isolated penalties, passed-pawn bonuses.
        for sq in board.pieces(Piece::Pawn) {
            if board.color_on(sq) != Some(color) {
                continue;
            }
            let file = sq.file() as usize;
            let rel_rank = sq.relative_to(color).rank() as usize;
            let abs_rank = sq.rank() as usize;
            if pawn_mask[ci][file].count_ones() > 1 {
                white_score -= sign * 11;
            }
            let left = if file == 0 { 0 } else { pawn_mask[ci][file - 1] };
            let right = if file == 7 { 0 } else { pawn_mask[ci][file + 1] };
            if left == 0 && right == 0 {
                white_score -= sign * 10;
            }
            // Passed if no enemy pawn stands ahead on this or an adjacent file.
            let ahead_mask = if color == Color::White {
                (!0u8).wrapping_shl((abs_rank + 1) as u32)
            } else {
                (1u8 << abs_rank) - 1
            };
            let mut passed = true;
            for ff in file.saturating_sub(1)..=(file + 1).min(7) {
                if pawn_mask[1 - ci][ff] & ahead_mask != 0 {
                    passed = false;
                    break;
                }
            }
            if passed {
                let r = rel_rank as i32;
                white_score += sign * (10 + r * r * 3);
                white_eg += sign * (12 + r * r * 5);
            }
        }

        // Bishop pair (stronger in the endgame).
        if bishops[ci] >= 2 {
            white_score += sign * 28;
            white_eg += sign * 40;
        }

        // Space: advanced pawns (middlegame only, small on purpose).
        white_score += sign * space[ci];

        // King danger: concrete pressure, not principles.
        let danger = king_danger(board, occ, color, king_sq[ci], &pawn_mask, &major_mask);
        white_score -= sign * danger;
        white_eg -= sign * danger * 2 / 5;

        // Flexibility: reward an uncommitted king while it is safe. This is
        // what lets the engine delay castling until danger makes it necessary.
        let king = king_sq[ci];
        let committed = matches!(king.file() as usize, 2 | 6); // c- or g-file
        let home_area = king.relative_to(color).rank() as usize <= 1;
        if !committed && home_area {
            let fade = (FLEX_FADE - danger).clamp(0, FLEX_FADE);
            white_score += sign * FLEXIBILITY * fade / FLEX_FADE;
        }

        // A king marching forward in the middlegame is a liability.
        let rel_rank = king.relative_to(color).rank() as usize;
        if rel_rank >= 3 {
            white_score -= sign * ((rel_rank - 2) as i32) * 10;
        }
    }

    mg += white_score;
    eg += white_eg;

    let phase = phase.clamp(0, TOTAL_PHASE);
    let score = (mg * phase + eg * (TOTAL_PHASE - phase)) / TOTAL_PHASE;
    let relative = if board.side_to_move() == Color::White {
        score
    } else {
        -score
    };
    relative + TEMPO
}

fn slider_mobility(from: Square, dirs: &[(i8, i8)], occ: BitBoard, own: BitBoard) -> i32 {
    let mut count = 0;
    for &(df, dr) in dirs {
        let mut cur = from;
        while let Some(next) = cur.try_offset(df, dr) {
            let target = next.bitboard();
            if occ & target == BitBoard::EMPTY {
                count += 1;
                cur = next;
                continue;
            }
            if own & target == BitBoard::EMPTY {
                count += 1; // capture on the blocking square
            }
            break;
        }
    }
    count
}

/// Concrete king-safety pressure for `color`, in centipawns.
///
/// Counts enemy pieces attacking the zone around the king, enemy majors and
/// open/semi-open files cutting through it. Cheap distance filter first so
/// quiet positions stay fast.
fn king_danger(
    board: &Board,
    occ: BitBoard,
    color: Color,
    king: Square,
    pawn_mask: &[[u8; 8]],
    major_mask: &[[u8; 8]],
) -> i32 {
    let ci = usize::from(color == Color::White);
    let mut zone = king.bitboard();
    for &(df, dr) in &KING_DELTAS {
        if let Some(t) = king.try_offset(df, dr) {
            zone |= t.bitboard();
        }
    }

    let mut danger = 0;
    for enemy in board.colors(!color) {
        let Some(piece) = board.piece_on(enemy) else {
            continue;
        };
        // Only pieces within striking range matter for melee pressure; long
        // range file pressure is handled by the file analysis below.
        let near = (enemy.file() as usize).abs_diff(king.file() as usize) <= 6
            && (enemy.rank() as usize).abs_diff(king.rank() as usize) <= 6;
        if near && attacks_zone(enemy, piece, !color, occ, zone) {
            danger += DANGER_PIECE[piece as usize];
            if zone & enemy.bitboard() != BitBoard::EMPTY {
                danger += DANGER_MELEE;
            }
        }
    }

    // Files cutting through the zone: missing shield, open lines, enemy majors.
    let mut zone_files = [false; 8];
    for sq in zone {
        zone_files[sq.file() as usize] = true;
    }
    for (file, &in_zone) in zone_files.iter().enumerate() {
        if !in_zone {
            continue;
        }
        if pawn_mask[ci][file] == 0 {
            if pawn_mask[1 - ci][file] == 0 {
                danger += DANGER_OPEN_FILE;
            } else {
                danger += DANGER_SEMI_OPEN_FILE;
            }
        }
        if major_mask[1 - ci][file] != 0 {
            danger += DANGER_ENEMY_MAJOR;
        }
    }
    danger
}

/// Does `from` (a piece of `color`) attack any square of `zone`?
fn attacks_zone(from: Square, piece: Piece, color: Color, occ: BitBoard, zone: BitBoard) -> bool {
    let hits = |sq: Square| sq.bitboard() & zone != BitBoard::EMPTY;
    match piece {
        Piece::Pawn => {
            let dir = if color == Color::White { 1i8 } else { -1i8 };
            [-1i8, 1].iter().any(|&df| from.try_offset(df, dir).is_some_and(hits))
        }
        Piece::Knight | Piece::King => {
            let deltas = if piece == Piece::Knight {
                &KNIGHT_DELTAS
            } else {
                &KING_DELTAS
            };
            deltas
                .iter()
                .any(|&(df, dr)| from.try_offset(df, dr).is_some_and(hits))
        }
        Piece::Bishop | Piece::Rook | Piece::Queen => {
            let dirs: &[(i8, i8)] = match piece {
                Piece::Bishop => &BISHOP_DIRS,
                Piece::Rook => &ROOK_DIRS,
                _ => &QUEEN_DIRS,
            };
            for &(df, dr) in dirs {
                let mut cur = from;
                while let Some(next) = cur.try_offset(df, dr) {
                    if hits(next) {
                        return true;
                    }
                    if occ & next.bitboard() != BitBoard::EMPTY {
                        break; // blocked, and the blocker itself was checked
                    }
                    cur = next;
                }
            }
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirror a FEN across the board (colors swapped, ranks reversed).
    fn mirror(fen: &str) -> String {
        let fields: Vec<&str> = fen.split_whitespace().collect();
        let flip = |c: char| match c {
            'P' => 'p',
            'N' => 'n',
            'B' => 'b',
            'R' => 'r',
            'Q' => 'q',
            'K' => 'k',
            'p' => 'P',
            'n' => 'N',
            'b' => 'B',
            'r' => 'R',
            'q' => 'Q',
            'k' => 'K',
            other => other,
        };
        let ranks: Vec<String> = fields[0]
            .split('/')
            .rev()
            .map(|r| r.chars().map(flip).collect())
            .collect();
        let side = if fields[1] == "w" { "b" } else { "w" };
        let castle: String = fields[2]
            .chars()
            .map(|c| match c {
                'K' => 'k',
                'Q' => 'q',
                'k' => 'K',
                'q' => 'Q',
                other => other,
            })
            .collect();
        // The en-passant square keeps its file but flips rank.
        let ep = if fields[3] == "-" {
            "-".to_string()
        } else {
            let mut chars = fields[3].chars();
            let file = chars.next().unwrap();
            let rank: char = chars.next().unwrap();
            format!("{file}{}", (b'9' - rank as u8 + b'0') as char)
        };
        format!(
            "{} {} {} {} {} {}",
            ranks.join("/"),
            side,
            castle,
            ep,
            fields[4],
            fields[5]
        )
    }

    fn eval(fen: &str) -> i32 {
        let board: Board = fen.parse().unwrap();
        evaluate(&board)
    }

    #[test]
    fn evaluation_is_symmetric_up_to_tempo() {
        let fens = [
            "r3k2r/pppppppp/8/8/8/8/PPPPPPPP/R3K2R w KQkq - 0 1",
            "rnbqkbnr/pp1ppppp/8/2p5/4P3/5N2/PPPP1PPP/RNBQKB1R b KQkq - 1 2",
            "4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 1",
            "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
        ];
        for fen in fens {
            let a = eval(fen);
            let b = eval(&mirror(fen));
            // Color symmetry: mirroring the position (which also flips the
            // side to move) must yield the same side-to-move-relative score;
            // the tempo bonus is applied to the side to move in both cases.
            assert_eq!(a, b, "asymmetric eval for {fen}: {a} vs mirrored {b}");
        }
    }

    #[test]
    fn material_dominates_style_terms() {
        // King and queen against a bare king: a huge, unmistakable advantage.
        assert!(eval("4k3/8/8/8/8/8/8/3QK3 w - - 0 1") > 400);
        assert!(eval("4k3/8/8/8/8/8/8/3QK3 b - - 0 1") < -400);
    }

    #[test]
    fn safe_uncastled_king_keeps_the_flexibility_bonus() {
        // Quiet position, no pressure: keeping the king uncommitted should be
        // at least as good as having already castled (the "creative" feature).
        let quiet_uncastled = eval("r3k2r/pppppppp/8/8/8/8/PPPPPPPP/R3K2R w KQkq - 0 1");
        let quiet_castled = eval("r4rk1/pppppppp/8/8/8/8/PPPPPPPP/R4RK1 w - - 0 1");
        assert!(
            quiet_uncastled >= quiet_castled,
            "uncastled {quiet_uncastled} should not be worse than castled {quiet_castled}"
        );
    }

    #[test]
    fn danger_makes_castling_necessary() {
        // Same structure, but a black rook stares down the e-file at the king:
        // now castling is concretely better (principles as emergency measure).
        let pressured = eval("4r1k1/pppppppp/8/8/8/8/PPPPPPPP/R3K2R w KQ - 0 1");
        let castled = eval("4r1k1/pppppppp/8/8/8/8/PPPPPPPP/R4RK1 w - - 0 1");
        assert!(
            castled > pressured,
            "castled {castled} should beat staying {pressured} under pressure"
        );
    }
}
