# Looking inside a document: the Info panel

View -> Document Info (Ctrl+I) opens the **Info** tab of the sidebar. It tells you what is in the file you are reading, which is useful when a document arrives from someone you do not know. The engine reads the file; Vellora shows the answer as plain text, so a title or an address in the file can never change how the panel looks.

## What it shows

| Section | What you see |
|---|---|
| **Properties** | File size, PDF version, number of pages, the size of the pages (a document with several sizes lists each, with how many pages have it), title, author, subject, keywords, the program that made the original and the one that made the PDF, creation and modification dates (as the file records them), whether the document is tagged for accessibility, and whether it is laid out for fast web view. |
| **Security** | Whether the file is encrypted (the method, the key length and which password opened it), what its author allows (print, copy, change, fill in forms, and so on: a request to viewers, not a lock), and the **risky or notable features** the file contains, each with a short explanation: scripts that run when the file opens, JavaScript in general, actions that start a program or send form data, links, embedded files, forms, layers and signatures. The addresses its links point to and the names of its embedded files are listed. |
| **Revisions** | How many times the file was saved on top of itself, and the bytes each version covers. |
| **Repairs** | Whether Vellora had to repair the file to read it, and why, and any problems it met. |

Hover a row to see its whole text. Vellora never runs a script, starts a program or opens an embedded file; the panel only tells you they are there. A signature is listed, not checked.

Lists are cut at 100 entries (and each entry at 1,024 characters) so that a hostile file cannot fill the panel; the panel says how many more there are.

## Saving the report

File -> Export Report as JSON… saves the same report as a file. It is exactly what the command `vellora inspect --json <file>` prints, so scripts can read both the same way; the format is described in [the command line guide](cli.md).

## Limits

- "No risky features found" means the scan did not find any in the places it looks (the catalog, every page and its annotations, the form fields and the bookmarks). If the scan was cut short, the panel says so.
- The report is made once, when the tab is first shown. If the file changes on disk, reopen it.
