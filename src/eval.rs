use cozy_chess::{Board, Color, Piece, Square};

const VALUES: [i32; 6] = [100, 320, 335, 500, 950, 0];

/// Side-to-move relative evaluation in centipawns.
pub fn evaluate(board: &Board) -> i32 {
    let mut white_score = 0;
    let mut pawns_by_file = [[0u8; 8]; 2];
    let mut pawn_squares = [Vec::with_capacity(8), Vec::with_capacity(8)];
    let mut bishops = [0u8; 2];
    let phase = board.occupied().len();

    for square in Square::ALL {
        let Some(piece) = board.piece_on(square) else {
            continue;
        };
        let color = board.color_on(square).expect("occupied square has a color");
        let ci = if color == Color::White { 0 } else { 1 };
        let sign = if color == Color::White { 1 } else { -1 };
        let relative = square.relative_to(color);
        let rank = relative.rank() as usize;
        let file = relative.file() as usize;
        let center = 7 - (file.abs_diff(3) + rank.abs_diff(3)).min(6);
        if piece == Piece::Pawn {
            pawns_by_file[ci][file] += 1;
            pawn_squares[ci].push(square);
        }
        if piece == Piece::Bishop {
            bishops[ci] += 1;
        }
        let positional = match piece {
            Piece::Pawn => rank as i32 * 7 + center as i32 * 2,
            Piece::Knight => center as i32 * 7,
            Piece::Bishop => center as i32 * 4,
            Piece::Rook => rank as i32 * 2,
            Piece::Queen => center as i32 * 2,
            Piece::King => {
                if phase < 14 {
                    center as i32 * 7
                } else {
                    -(center as i32) * 3
                }
            }
        };
        white_score += sign * (VALUES[piece as usize] + positional);
    }

    // Reward a bishop pair and penalize doubled, isolated, and blockaded passers.
    for color in Color::ALL {
        let ci = if color == Color::White { 0 } else { 1 };
        let sign = if color == Color::White { 1 } else { -1 };
        if bishops[ci] >= 2 {
            white_score += sign * 28;
        }
        for &pawn in &pawn_squares[ci] {
            let file = pawn.file() as usize;
            let rank = pawn.relative_to(color).rank() as usize;
            if pawns_by_file[ci][file] > 1 {
                white_score -= sign * 11;
            }
            let isolated = (file == 0 || pawns_by_file[ci][file - 1] == 0)
                && (file == 7 || pawns_by_file[ci][file + 1] == 0);
            if isolated {
                white_score -= sign * 10;
            }
            let blocked = pawn_squares[1 - ci].iter().any(|&enemy| {
                (enemy.file() as usize).abs_diff(file) <= 1
                    && enemy.relative_to(color).rank() as usize > rank
            });
            if !blocked {
                white_score += sign * (10 + rank as i32 * rank as i32 * 3);
            }
        }
    }

    let relative = if board.side_to_move() == Color::White {
        white_score
    } else {
        -white_score
    };
    relative + 12
}
