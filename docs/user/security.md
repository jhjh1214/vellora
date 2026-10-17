# Security: what the security report means

Vellora assumes that any PDF can be hostile. Two things protect you, and they are different:

1. **How Vellora opens a file.** The file is read and drawn by a separate program, the engine, that has no network access and cannot reach your other files (on Windows and Linux; see [Privacy](privacy.md)). Scripts, programs and attachments in a document are never run or opened. This happens whether or not you look at any report.
2. **What Vellora tells you about the file.** The security report says which *features* a file contains that could be a problem in some other program, so you can decide whether to trust it, forward it or open it elsewhere. This page is about that report.

## Where you see it

- **In the application:** View -> Document Info (Ctrl+I) opens the Info panel. Its **Security** section lists whether the file is encrypted, what its author allows, and the risky or notable features it contains. See [the Info panel](inspector.md).
- **On the command line:** `vellora security <file>` gives a verdict and exit code you can use in a script, and `vellora inspect <file>` gives the full report. See [the command line](cli.md#vellora-security).

Both read the same facts from the file and neither opens or runs anything in it.

## What the verdict says

`vellora security` prints one of:

- **no risky features found.** The scan looked in the places where these features live (the catalog, every page and its annotations, form fields and bookmarks) and found none of the features below.
- **RISKY: n risky features found.** It lists each one with a sentence of explanation.

The exit code is 0 for the first, 3 for the second, and 1 if the file could not be read or assessed.

### The features that count as risky

| Finding | What it means |
|---|---|
| Runs a script when the file is opened | JavaScript that other viewers may run as soon as the document opens. |
| Contains JavaScript actions | Scripts attached to links, buttons, form fields or bookmarks. |
| Has document-level scripts | Scripts stored with the document, which some viewers run on opening. |
| Reacts to events | Actions that fire when the document is opened, closed or printed, or a form field is entered. |
| Can start a program or open a file | A launch action: it asks a viewer to start another program or open a file on your computer. |
| Sends form data to an address | A form that can send what you typed to a server. |
| Opens other PDF files | A link that opens another PDF document. |
| Has embedded files | Files attached inside the PDF. Opening one can run whatever program handles its type. |
| Has an XFA form | A form in an older XML-based format that can carry scripts. |

Vellora does none of these things itself: it never runs PDF JavaScript, never follows a launch action, never submits a form and never opens an attachment. The findings are warnings about what *another* program might do with the same file.

### Also present

Some features are listed as "also present" but do not make a file risky on their own: links to addresses, an interactive form, layers, and signature fields. The addresses of the links and the names of the embedded files are listed so that you can read them. A signature is listed but **not checked**: Vellora does not say whether it is valid.

## What the report does not say

- **"No risky features found" does not mean "safe".** It means none of the features above were found. A file can still be built to exploit a bug in a viewer; that is what the engine's isolation is for.
- **An incomplete scan says so.** If the scan hit its work limit or could not read part of the file, the report says that a feature may have been missed, and when it found nothing `vellora security` exits with 1 instead of 0. If the file had to be repaired to be read, the report says the scan may have missed something too.
- **What the author allows is a request, not a lock.** A document can say it may not be printed or copied. Vellora follows that for printing (see [Printing](printing.md)), but these permissions are not a security boundary; the report shows them as information.
- **Text from the file is shown as plain text.** Titles, addresses and names are cut to a fixed length and shown without formatting, and in the terminal control characters are replaced, so a hostile file cannot change how the report looks or move your cursor.
- **An encrypted file** needs its password before it can be scanned.
