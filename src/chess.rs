use cozy_chess::{util, Board, Move, MoveParseError, Piece};

/// Parse standard UCI notation, translating castling into cozy-chess's internal form.
pub fn parse_uci_move(board: &Board, text: &str) -> Result<Move, MoveParseError> {
    util::parse_uci_move(board, text)
}

/// Format a move in standard UCI notation (e1g1 for kingside castling).
pub fn format_uci_move(board: &Board, mv: Move) -> String {
    util::display_uci_move(board, mv).to_string()
}

pub fn is_en_passant(board: &Board, mv: Move) -> bool {
    board.piece_on(mv.from) == Some(Piece::Pawn)
        && board.piece_on(mv.to).is_none()
        && board.en_passant() == Some(mv.to.file())
}

/// cozy-chess encodes castling as the king moving onto its own rook's square
/// (king-captures-rook). Detect that pattern so castling is never mistaken
/// for a capture or an attack on the rook.
pub fn is_castling(board: &Board, mv: Move) -> bool {
    board.piece_on(mv.from) == Some(Piece::King) && board.color_on(mv.to) == board.color_on(mv.from)
}

pub fn is_capture(board: &Board, mv: Move) -> bool {
    match board.piece_on(mv.to) {
        // A same-colored piece on the target square means castling, not a capture.
        Some(_) => board.color_on(mv.to) != board.color_on(mv.from),
        None => is_en_passant(board, mv),
    }
}

pub fn format_pv(board: &Board, moves: &[Move]) -> String {
    let mut position = board.clone();
    let mut formatted = Vec::with_capacity(moves.len());
    for &mv in moves {
        formatted.push(format_uci_move(&position, mv));
        position.play_unchecked(mv);
    }
    formatted.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cozy_chess::{Piece, Square};
    #[test]
    fn standard_castling_notation_is_parsed_and_printed() {
        let board: Board = "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1".parse().unwrap();
        let mv = parse_uci_move(&board, "e1g1").unwrap();
        assert_eq!(mv.to, Square::H1);
        assert_eq!(format_uci_move(&board, mv), "e1g1");

        let black: Board = "r3k2r/8/8/8/8/8/8/R3K2R b KQkq - 0 1".parse().unwrap();
        let mv = parse_uci_move(&black, "e8c8").unwrap();
        assert_eq!(mv.to, Square::A8);
        assert_eq!(format_uci_move(&black, mv), "e8c8");
    }

    #[test]
    fn en_passant_is_a_capture_and_removes_the_pawn() {
        let mut board: Board = "4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 1".parse().unwrap();
        let mv = parse_uci_move(&board, "e5d6").unwrap();
        assert!(is_en_passant(&board, mv));
        assert!(is_capture(&board, mv));
        board.play_unchecked(mv);
        assert_eq!(board.piece_on(Square::D6), Some(Piece::Pawn));
        assert_eq!(board.piece_on(Square::D5), None);
        assert_eq!(board.piece_on(Square::E5), None);
    }

    #[test]
    fn castling_is_not_a_capture() {
        // cozy-chess encodes castling as king-takes-rook; the target square
        // holds our own rook, which must not be counted as a capture.
        let board: Board = "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1".parse().unwrap();
        let white_kingside = parse_uci_move(&board, "e1g1").unwrap();
        assert!(is_castling(&board, white_kingside));
        assert!(!is_capture(&board, white_kingside));

        let white_queenside = parse_uci_move(&board, "e1c1").unwrap();
        assert!(is_castling(&board, white_queenside));
        assert!(!is_capture(&board, white_queenside));

        let black: Board = "r3k2r/8/8/8/8/8/8/R3K2R b KQkq - 0 1".parse().unwrap();
        let black_kingside = parse_uci_move(&black, "e8g8").unwrap();
        assert!(is_castling(&black, black_kingside));
        assert!(!is_capture(&black, black_kingside));

        // Real captures still count.
        let capture: Board = "r3k2r/8/8/8/8/8/8/R3KQ1R w KQkq - 0 1".parse().unwrap();
        let rxh8 = parse_uci_move(&capture, "h1h8").unwrap();
        assert!(!is_castling(&capture, rxh8));
        assert!(is_capture(&capture, rxh8));

        // Quiet moves stay quiet.
        let quiet = parse_uci_move(&board, "a1b1").unwrap();
        assert!(!is_castling(&board, quiet));
        assert!(!is_capture(&board, quiet));
    }
}
