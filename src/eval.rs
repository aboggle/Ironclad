//! A tapered evaluation of material, piece-square values, mobility, pawns
//! (structure: doubled, isolated, backward, connected, open files, holes,
//! passed), bishop pair and blocked bishops, outposts, space and king safety.

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
const ROOK_OPEN_FILE: i32 = 12;
const ROOK_SEMI_OPEN_FILE: i32 = 7;
const ROOK_SEVENTH_RANK: i32 = 12;
const ROOK_OPEN_FILE_EG: i32 = 6;
const ROOK_SEMI_OPEN_FILE_EG: i32 = 4;
const ROOK_SEVENTH_RANK_EG: i32 = 8;

/// Doubled-pawn penalty (per extra pawn on the file).
const DOUBLED_PENALTY: i32 = 11;
/// Isolated-pawn penalty.
const ISOLATED_PENALTY: i32 = 10;
/// Backward-pawn penalty.
const BACKWARD_PENALTY: i32 = 10;
/// Pawn sitting in an enemy pawn chain: attacked by enemy pawns, nothing of
/// ours defends it.
const HOLE_PENALTY: i32 = 8;
/// Bonus for own pawns within one rank on adjacent files ([mg, eg] per pawn).
const CONNECTED_BONUS: [i32; 2] = [5, 6];
/// Bonus for a pawn on a file with no enemy pawns ([mg, eg] per pawn).
const OPEN_FILE_PAWN: [i32; 2] = [2, 1];
/// Outpost bonus [mg, eg] for knights / bishops on the enemy half.
const OUTPOST_KNIGHT: [i32; 2] = [12, 8];
const OUTPOST_BISHOP: [i32; 2] = [8, 6];
/// Own pawns blocking a bishop's diagonal: [mg, eg] per bishop.
const BISHOP_BLOCKED_PENALTY: [i32; 2] = [6, 3];
/// Bishop pair (stronger in the endgame).
const BISHOP_PAIR: [i32; 2] = [28, 40];

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

/// Piece-square values by piece ([P, N, B, R, Q, K][rank][file]), white-relative.
const MG_PST: [[[i32; 8]; 8]; 6] = [
    // Pawn
    [
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 1, 2, 3, 3, 2, 1, 0],
        [0, 2, 4, 6, 6, 4, 2, 0],
        [0, 3, 6, 9, 9, 6, 3, 0],
        [0, 4, 8, 12, 12, 8, 4, 0],
        [0, 5, 10, 15, 15, 10, 5, 0],
        [0, 6, 12, 18, 18, 12, 6, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ],
    // Knight
    [
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 2, 2, 2, 2, 0, 0],
        [0, 0, 3, 4, 4, 3, 0, 0],
        [0, 0, 4, 5, 6, 4, 0, 0],
        [0, 0, 4, 5, 5, 4, 0, 0],
        [0, 0, 2, 2, 2, 2, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ],
    // Bishop
    [
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 1, 1, 1, 1, 1, 1, 0],
        [0, 1, 2, 2, 2, 2, 1, 0],
        [0, 2, 3, 3, 3, 3, 2, 0],
        [0, 3, 4, 4, 4, 4, 3, 0],
        [0, 3, 4, 5, 5, 4, 3, 0],
        [0, 2, 3, 3, 3, 3, 2, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ],
    // Rook
    [
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 2, 3, 4, 4, 3, 2, 0],
        [0, 4, 6, 8, 8, 6, 4, 0],
        [0, 6, 9, 12, 12, 9, 6, 0],
        [0, 9, 12, 14, 14, 12, 9, 0],
        [0, 12, 14, 16, 16, 14, 12, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ],
    // Queen
    [
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 2, 3, 3, 3, 3, 2, 0],
        [0, 4, 7, 9, 9, 7, 4, 0],
        [0, 6, 10, 12, 12, 10, 6, 0],
        [0, 9, 13, 16, 16, 13, 9, 0],
        [0, 12, 15, 18, 18, 15, 12, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ],
    // King: home-square penalty (-20 at E1, -10 at G1, 0 elsewhere); black
    // kings get the mirrored reward from the color sign flip.
    [
        [0, 0, 0, 0, -20, 0, -10, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 1, 2, 2, 1, 0, 0],
        [0, 0, 2, 4, 4, 2, 0, 0],
        [0, 0, 2, 4, 4, 2, 0, 0],
        [0, 0, 1, 2, 2, 1, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ],
];

/// Piece-square values by piece ([P, N, B, R, Q, K][rank][file]), white-relative.
const EG_PST: [[[i32; 8]; 8]; 6] = [
    // Pawn
    [
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 1, 2, 3, 3, 2, 1, 0],
        [0, 2, 4, 5, 5, 4, 2, 0],
        [0, 3, 5, 7, 7, 5, 3, 0],
        [0, 4, 6, 8, 8, 6, 4, 0],
        [0, 5, 7, 9, 9, 7, 5, 0],
        [0, 6, 8, 10, 10, 8, 6, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ],
    // Knight
    [
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 2, 3, 3, 2, 0, 0],
        [0, 0, 3, 5, 5, 3, 0, 0],
        [0, 0, 4, 6, 6, 4, 0, 0],
        [0, 0, 4, 6, 6, 4, 0, 0],
        [0, 0, 2, 3, 3, 2, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ],
    // Bishop
    [
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 1, 1, 1, 1, 1, 1, 0],
        [0, 1, 3, 3, 3, 3, 1, 0],
        [0, 2, 4, 5, 5, 4, 2, 0],
        [0, 3, 5, 7, 7, 5, 3, 0],
        [0, 3, 5, 7, 7, 5, 3, 0],
        [0, 2, 3, 4, 4, 3, 2, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ],
    // Rook
    [
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 2, 3, 4, 4, 3, 2, 0],
        [0, 4, 6, 8, 8, 6, 4, 0],
        [0, 6, 9, 12, 12, 9, 6, 0],
        [0, 9, 12, 15, 15, 12, 9, 0],
        [0, 12, 15, 18, 18, 15, 12, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ],
    // Queen
    [
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 2, 4, 5, 5, 4, 2, 0],
        [0, 4, 8, 11, 11, 8, 4, 0],
        [0, 6, 12, 15, 15, 12, 6, 0],
        [0, 9, 15, 18, 18, 15, 9, 0],
        [0, 12, 17, 20, 20, 17, 12, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ],
    // King
    [
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 1, 2, 2, 1, 0, 0],
        [0, 0, 2, 4, 4, 2, 0, 0],
        [0, 0, 3, 6, 6, 3, 0, 0],
        [0, 0, 4, 6, 6, 4, 0, 0],
        [0, 0, 3, 5, 5, 3, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0],
    ],
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
    let mut pawns_of = [BitBoard::EMPTY; 2];

    // Single pass over all pieces: material, piece-square, mobility, structure.
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

        phase += PHASE_W[piece as usize];
        mg += sign * MG_VAL[piece as usize];
        eg += sign * EG_VAL[piece as usize];

        // Piece-square values are stored white-relative; the sign makes them
        // follow the color.
        mg += sign * MG_PST[piece as usize][rank][file];
        eg += sign * EG_PST[piece as usize][rank][file];

        if piece == Piece::Pawn {
            pawn_mask[ci][abs_file] |= 1 << abs_rank;
            pawns_of[ci] |= square.bitboard();
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

    // Squares attacked by pawns of each color (white-relative tables indexed by ci).
    let pawn_attacks = [
        pawn_attacks(Color::White, pawns_of[0]),
        pawn_attacks(Color::Black, pawns_of[1]),
    ];

    // Pawn structure, bishop activity and king safety, per color (white-positive).
    let mut white_score = 0i32;
    let mut white_eg = 0i32;
    for color in Color::ALL {
        let ci = usize::from(color == Color::White);
        let sign = if color == Color::White { 1 } else { -1 };
        let pawns = pawns_of[ci];

        // Doubled pawns: 2+ pawns on a file cost an extra 11 each.
        for file in &pawn_mask[ci] {
            let count = file.count_ones() as i32;
            white_score -= sign * count.saturating_sub(1) * DOUBLED_PENALTY;
        }

        for sq in pawns {
            let file = sq.file() as usize;
            let abs_rank = sq.rank() as usize;

            // Isolated: nothing on either adjacent file.
            if (file == 0 || pawn_mask[ci][file - 1] == 0)
                && (file == 7 || pawn_mask[ci][file + 1] == 0)
            {
                white_score -= sign * ISOLATED_PENALTY;
            }

            // Backward: no own pawn within one rank on an adjacent file, while
            // the enemy defends the square this pawn sits on.
            let supported = (file > 0 && nearby(pawn_mask[ci][file - 1], abs_rank))
                || (file < 7 && nearby(pawn_mask[ci][file + 1], abs_rank));
            // White pawns are defended by enemy pawns at abs_rank + 1; black
            // pawns at abs_rank - 1.
            let defend_rank = if color == Color::White {
                Some(abs_rank + 1)
            } else {
                abs_rank.checked_sub(1)
            };
            let defended = defend_rank.is_some_and(|r| {
                r < 8
                    && ((file > 0 && (pawn_mask[1 - ci][file - 1] & (1u8 << r)) != 0)
                        || (file < 7 && (pawn_mask[1 - ci][file + 1] & (1u8 << r)) != 0))
            });
            if !supported && defended {
                white_score -= sign * BACKWARD_PENALTY;
            }

            // Connected: own pawn within one rank on an adjacent file.
            if (file > 0 && nearby(pawn_mask[ci][file - 1], abs_rank))
                || (file < 7 && nearby(pawn_mask[ci][file + 1], abs_rank))
            {
                white_score += sign * CONNECTED_BONUS[0];
                white_eg += sign * CONNECTED_BONUS[1];
            }

            // Passed: no enemy pawn ahead on this or an adjacent file.
            let ahead_mask = forward_mask(color, abs_rank);
            if [file.saturating_sub(1), file, (file + 1).min(7)]
                .into_iter()
                .all(|f| pawn_mask[1 - ci][f] & ahead_mask == 0)
            {
                let r = sq.relative_to(color).rank() as i32;
                white_score += sign * (10 + r * r * 3);
                white_eg += sign * (12 + r * r * 5);
            }

            // Open file: no enemy pawn anywhere on this file.
            if pawn_mask[1 - ci][file] == 0 {
                white_score += sign * OPEN_FILE_PAWN[0];
            }
        }

        // Holes: our pawns in enemy pawn chains — attacked by enemy pawns but
        // not defended by our own pawns.
        let holes = pawns & pawn_attacks[1 - ci] & !pawn_attacks[ci];
        white_score -= sign * holes.len() as i32 * HOLE_PENALTY;

        // Outposts: minors on the enemy half, unreachable by enemy pawns.
        for (piece, bonus) in [
            (Piece::Knight, OUTPOST_KNIGHT),
            (Piece::Bishop, OUTPOST_BISHOP),
        ] {
            for sq in board.pieces(piece) {
                if board.color_on(sq) != Some(color) {
                    continue;
                }
                let rank = sq.relative_to(color).rank() as usize;
                if !(3..=5).contains(&rank)
                    || (pawn_attacks[1 - ci] & sq.bitboard()) != BitBoard::EMPTY
                {
                    continue;
                }
                white_score += sign * bonus[0];
                white_eg += sign * bonus[1];
            }
        }

        // Blocked bishops: own pawns on the bishop's diagonals.
        for sq in board.pieces(Piece::Bishop) {
            if board.color_on(sq) != Some(color) {
                continue;
            }
            let blocked = pawns
                .iter()
                .filter(|p| {
                    (p.file() as usize).abs_diff(sq.file() as usize)
                        == (p.rank() as usize).abs_diff(sq.rank() as usize)
                })
                .count();
            if blocked > 0 {
                white_score -= sign * BISHOP_BLOCKED_PENALTY[0];
                white_eg -= sign * BISHOP_BLOCKED_PENALTY[1];
            }
        }

        // Bishop pair (stronger in the endgame).
        if bishops[ci] >= 2 {
            white_score += sign * BISHOP_PAIR[0];
            white_eg += sign * BISHOP_PAIR[1];
        }

        // Space: advanced pawns, middlegame only (small on purpose).
        white_score += sign * space[ci];

        // Rooks benefit from open files and the seventh rank. Keep the
        // bonuses modest so tactics and material dominate.
        for rook in board.pieces(Piece::Rook) {
            if board.color_on(rook) != Some(color) {
                continue;
            }
            let file = rook.file() as usize;
            if pawn_mask[ci][file] == 0 {
                if pawn_mask[1 - ci][file] == 0 {
                    white_score += sign * ROOK_OPEN_FILE;
                    white_eg += sign * ROOK_OPEN_FILE_EG;
                } else {
                    white_score += sign * ROOK_SEMI_OPEN_FILE;
                    white_eg += sign * ROOK_SEMI_OPEN_FILE_EG;
                }
            }
            if rook.relative_to(color).rank() as usize == 6 {
                white_score += sign * ROOK_SEVENTH_RANK;
                white_eg += sign * ROOK_SEVENTH_RANK_EG;
            }
        }

        // King danger: concrete pressure, not principles.
        let danger = king_danger(board, occ, color, king_sq[ci], &pawn_mask, &major_mask);
        white_score -= sign * danger;
        white_eg -= sign * danger * 2 / 5;
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

/// Squares attacked by the pawns of `color`.
fn pawn_attacks(color: Color, pawns: BitBoard) -> BitBoard {
    let dr = if color == Color::White { 1 } else { -1 };
    let mut bb = BitBoard::EMPTY;
    for sq in pawns {
        for df in [-1i8, 1] {
            if let Some(t) = sq.try_offset(df, dr) {
                bb |= t.bitboard();
            }
        }
    }
    bb
}

/// Does a file mask have a pawn within one rank of `rank`?
fn nearby(file_mask: u8, rank: usize) -> bool {
    (rank.saturating_sub(1)..=(rank + 1).min(7)).any(|r| file_mask & (1u8 << r) != 0)
}

/// Mask of absolute ranks in front of a pawn (ranks the pawn advances into).
fn forward_mask(color: Color, abs_rank: usize) -> u8 {
    if color == Color::White {
        if abs_rank >= 7 {
            0
        } else {
            u8::MAX << (abs_rank + 1)
        }
    } else {
        (1u8 << abs_rank) - 1
    }
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
            [-1i8, 1]
                .iter()
                .any(|&df| from.try_offset(df, dir).is_some_and(hits))
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
    fn castled_kings_are_safer_in_the_middlegame() {
        let uncastled = eval("r4rk1/pppppppp/8/8/8/8/PPPPPPPP/R3K2R w KQ - 0 1");
        let castled = eval("r3k2r/pppppppp/8/8/8/8/PPPPPPPP/R4RK1 w kq - 0 1");
        assert!(
            castled > uncastled,
            "castled {castled} should beat uncastled {uncastled}"
        );
    }

    #[test]
    fn danger_makes_castling_necessary() {
        // Equal material; a black rook stares down the e-file at the white
        // king, so moving the king away from the pressure should help.
        let pressured = eval("r3r1k1/pppppppp/8/8/8/8/PPPPPPPP/R3K2R w - - 0 1");
        let castled = eval("r3r1k1/pppppppp/8/8/8/8/PPPPPPPP/R4RK1 w - - 0 1");
        assert!(
            castled > pressured,
            "castled {castled} should beat staying {pressured} under pressure"
        );
    }

    #[test]
    fn connected_pawns_beat_isolated_ones() {
        // White pawns at D5 and C4: each is within one rank of an own pawn
        // on an adjacent file, so both are connected.
        let connected = eval("8/8/8/3P4/2P5/3k4/8/5K2 w - - 0 1");
        // Same white pawn on D5 alone (no adjacent-file neighbor): isolated.
        let isolated = eval("8/8/8/3P4/8/3k4/8/5K2 w - - 0 1");
        assert!(
            connected > isolated,
            "connected {connected} should beat isolated {isolated}"
        );
    }

    #[test]
    fn passed_pawns_are_credited_extra() {
        // White pawn on D5 with no pawns ahead: passed (bonus 10 + 4^2 * 3).
        let passed = eval("8/8/8/3k4/8/3P4/8/3K4 w - - 0 1");
        // Black pawn at D6 sits ahead of the white pawn on D5: it blocks the
        // white pawn and the white pawn also sits in an enemy pawn chain.
        let blocked = eval("8/8/8/3p4/3P4/3k4/8/3K4 w - - 0 1");
        assert!(
            passed > blocked + 100,
            "passed {passed} should beat blocked {blocked} + 100"
        );
    }

    #[test]
    fn bishop_pair_is_rewarded() {
        let pair = eval("8/8/8/3k4/8/8/8/3BBK2 w - - 0 1");
        let solo = eval("8/8/8/3k4/8/8/8/3B1K2 w - - 0 1");
        assert!(
            pair - solo > 300,
            "bishop pair {pair} should beat one bishop {solo} by > 300"
        );
    }
}
