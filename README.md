# Ironclad

Ironclad is a UCI-compatible chess engine written in Rust. It uses `cozy-chess` for fast,car legal bitboard move generation and implements its own iterative-deepening principal-variation search.

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
- Tapered material and piece-square evaluation (white-relative PSTs) with mobility, pawn structure (doubled, isolated, backward, connected, open-file, holes, passed), outposts, blocked bishops, space, king danger, and a home-square king penalty.
- Iterative deepening, aspiration windows, principal-variation search, alpha-beta pruning, and quiescence search.
- Search improvements: singular extension for singular PV moves, counter-move heuristic, adaptive late-move reductions (history-based, losing-exchange captures), and SEE-based quiescence delta pruning.
- Exchange-aware capture ordering, promotions, hash/PV moves, killers, counter moves, and rewarded/penalized quiet-move history.
- Persistent transposition table with exact/lower/upper bounds and mate-distance normalization; supports `go nodes N`.
- Repetition and fifty-move draw handling in both full-width and quiescence search; pruning avoids checks and low-material zugzwang positions.
- Asynchronous search so the UCI loop can receive `stop` while thinking; progress reported via `info` lines with depth, seldepth, score, nodes, nps, hashfull, time, and pv.

## License

MIT. The move-generation dependency is separately licensed; see its crate metadata.
