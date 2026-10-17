# Privacy: what Vellora never does

People open confidential documents, so these are promises about what Vellora does *not* do. They come from the project's rules ([ADR-0012](https://github.com/jhjh1214/vellora/blob/main/docs/adr/0012-no-telemetry-local-crash-dumps.md) and the [security model](https://github.com/jhjh1214/vellora/blob/main/docs/architecture/security-model.md)), and the program is built to keep them.

## Vellora never

- **Sends anything about you or your documents.** There is no telemetry, no usage statistics and no automatic upload of crash reports.
- **Uses the network to read a document.** The part of Vellora that reads PDF files, the *engine*, is a separate program with no network code. On Windows it runs in a container without network access, and on Linux a filter refuses it sockets. The sandbox for macOS is not built yet, so there the promise rests on the engine having no network code.
- **Runs JavaScript from a PDF.** The PDF library is built without a JavaScript engine. Scripts in a file are reported (see [Security](security.md)), never run.
- **Runs launch actions.** A PDF can ask the reader to start a program. Vellora never does, and says that it did not.
- **Opens attachments.** Files embedded in a PDF are listed by name in the Info panel and in the reports of the command line. They are never opened or run.
- **Follows a web link without asking.** A link to a web or e-mail address shows the whole address and waits for your answer. Other kinds of address are refused.
- **Changes the file you are reading.** Vellora opens documents for reading only. A document that is damaged is repaired in memory, not on disk.

## What Vellora keeps on your computer

| What | Where | Notes |
|---|---|---|
| Settings and recent files | Your user settings | You can clear the recent list in File -> Open Recent. |
| Logs | A folder of your own, at most 10 MiB in all (Help -> Open Log Folder) | They never contain the text of a document or a password. |
| Crash reports | A folder beside the logs | Written only on your computer: what the program was doing (the call stacks of its threads), not the contents of memory. After a crash Vellora offers to show the folder and to open a prefilled issue form in your browser. Nothing is attached or uploaded; you choose what to attach, and issues on the project's site are public. |

A password you type for an encrypted document goes to the engine to open the document and nowhere else: it is not logged, not saved to disk and not put in the recent files list.

## The command line

The `vellora` command follows the same rules: it never runs or opens anything found inside a document, and it makes no network connections ([the command line](cli.md)).
