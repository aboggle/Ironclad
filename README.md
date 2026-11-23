# Ironclad

Ironclad is a UCI-compatible chess engine written in Rust. It uses `cozy-chess` for fast, legal bitboard move generation and implements its own iterative-deepening principal-variation search.

## Build

```sh
cargo build --release
```

The executable is `target/release/ironclad` (or `ironclad.exe` on Windows). Add it to any UCI-compatible chess GUI such as Cute Chess or Arena.

## UCI

Ironclad supports `uci`, `isready`, `ucinewgame`, `position startpos|fen ... moves ...`, and `go` with depth, movetime, clock/increment, movestogo, and infinite limits. A bare `go` continues searching until `stop`. Standard UCI castling notation and en-passant moves are supported.

Options:

- `Hash` (default 64 MB, range 1-2048)
- `Move Overhead` (default 20 ms)

Typical terminal session:

```text
uci
isready
position startpos
go depth 12
```

## Engine design

- Legal move generation and incremental Zobrist hashing from cozy-chess.
- Iterative deepening, aspiration windows, principal-variation search, alpha-beta pruning, and quiescence search.
- Persistent transposition table with exact/lower/upper bounds and mate-distance normalization.
- Exchange-aware capture ordering, promotions, hash/PV moves, killers, and rewarded/penalized quiet-move history.
- Tapered material and piece-square evaluation with mobility, king safety, rook activity, bishop-pair, and pawn-structure terms.
- Repetition and fifty-move draw handling in both full-width and quiescence search; pruning avoids checks and low-material zugzwang positions.
- Asynchronous search so the UCI loop can receive `stop` while thinking.

## License

MIT. The move-generation dependency is separately licensed; see its crate metadata.
