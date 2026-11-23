# Ironclad — agent instructions

Single-crate Rust UCI chess engine (`ironclad`, edition 2021). One binary entrypoint: `src/main.rs` → `uci::run()`. Depends only on `cozy-chess` 0.3.4.

## Commands

- Build: `cargo build --release` → `target/release/ironclad.exe` (Windows) / `target/release/ironclad`
- Run tests: `cargo test` (7 inline unit tests in `src/chess.rs` and `src/eval.rs`)
- Check: `cargo check`
- Format: `cargo fmt`
- Lint: `cargo clippy -- -D warnings`
- UCI smoke test: pipe commands into the binary, e.g. `echo -e "uci\nisready\nposition startpos\ngo depth 12\nquit" | target\release\ironclad.exe`

Release profile is optimized for strength over debuggability: `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`. Expect slow release builds; use `cargo build` (debug) for quick iteration.

## Search flow

UCI loop (`src/uci.rs`) spawns an asynchronous search (`src/search.rs`) so `stop`/`ponderhit` can interrupt thinking. Search uses iterative deepening, aspiration windows, PVS, quiescence, TT with bound types, MVV-LVA/killer/history ordering. Evaluation (`src/eval.rs`) is material + PST + bishop pair + pawn terms. Bitboard move generation/Zobrist hashing come from `cozy-chess` (`src/chess.rs`).

## Conventions / gotchas

- `/target`, `*.pgn`, and `*.log` are gitignored; do not commit them.
- No CI, no workspace, no feature flags, no env vars required.
- `panic = "abort"`: avoid `unwrap()`/`expect()` in new hot-path code where a panic would kill the engine mid-search.
- UCI options: `Hash` (1–2048 MB, default 64), `Move Overhead` (ms, default 20).
