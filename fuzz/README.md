# fuzz/ — fuzz targets

Created in **M0 task 13**. A standalone `cargo-fuzz` crate (not a workspace member; it needs the nightly toolchain).

Initial targets: `lexer`, `object_parser`, `xref_open`, `write_roundtrip`.

```sh
cd fuzz
cargo +nightly fuzz run xref_open -- -max_total_time=60
```

Crashes found by fuzzing are fixed together with a regression test in the affected crate. Generated corpora and artifacts are git-ignored. Security-relevant reproducers go to the restricted corpus (see SECURITY.md).
