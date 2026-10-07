# fuzz/ — fuzz targets

A standalone `cargo-fuzz` crate (`vellora-fuzz`, not a workspace member; it needs the nightly toolchain and Linux or macOS). Its own `Cargo.lock` is committed.

| Target | What it feeds |
|---|---|
| `lexer` | arbitrary bytes through `Lexer` (must not panic or stall) |
| `object_parser` | arbitrary bytes as a direct and as an indirect object, then re-serialised |
| `xref_open` | arbitrary bytes as a whole document (`ObjectStore::open`, then trailer, page walk and objects) |
| `write_roundtrip` | open → full rewrite (table and packed) and incremental update → reopen with the same pages |

```sh
cd fuzz
cargo +nightly fuzz run xref_open -- ../tests/corpus-data -max_total_time=60
```

`../tests/corpus-data` (from `cargo xtask corpus fetch`) is an optional seed corpus: real files reach much deeper than random bytes. CI runs every target for 60 s per pull request; `nightly.yml` runs each for 30 min.

Crashes found by fuzzing are fixed together with a regression test in the affected crate. Generated corpora and artifacts are git-ignored. Security-relevant reproducers go to the restricted corpus (see SECURITY.md).
