# Install

Vellora has no released version yet, so there is no installer to download. Until there is one, you build it from the source code.

## Building from source

The tools you need on Windows, Linux and macOS (a Rust toolchain, a C++20 compiler, CMake and Qt 6.8) and the commands are in the repository:

- [README, "Building"](https://github.com/jhjh1214/vellora#building)
- [Development setup](https://github.com/jhjh1214/vellora/blob/main/docs/dev/setup.md)

The program is made of two executables that must stay in the same folder: `vellora` (the window you use) and `vellora-engine` (which reads the PDF; see [Privacy](privacy.md)), together with the PDFium library the engine draws pages with.

## Running it

Start `vellora`, or give it files to open:

```
vellora report.pdf notes.pdf
```

Each file opens in its own tab. If Vellora is already running, the files open in that window instead of starting a second program.

## Where Vellora keeps things

- **Settings and the list of recent files** are in your user settings, in the format your system uses (the registry on Windows, a property list on macOS, a configuration file on Linux).
- **Logs** are in a folder of your own, at most 10 MiB in all. Help -> Open Log Folder shows it. Logs do not contain the text of your documents or any password.
- **Crash reports**, if the program crashes, are written to a local folder too. Nothing is sent anywhere; see [Privacy](privacy.md).
