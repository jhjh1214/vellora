# First steps

## Open a document

Any of these:

- File -> Open… (Ctrl+O) and choose a PDF.
- Drop one or more files on the window.
- File -> Open Recent, for files you opened before. "Clear Recent Files" and "Remove Unavailable Files" tidy the list.
- Give the files on the command line: `vellora a.pdf b.pdf`.

Every file opens in its own tab. Ctrl+Tab and Ctrl+Shift+Tab move between tabs, File -> Close Tab closes one, and File -> Reopen Closed Tab brings back the last one you closed.

If the document is protected by a password, Vellora asks for it. A wrong password lets you try again a few times. The password is used to open the document and is not written to the log, to disk or to the list of recent files.

If a file is damaged and Vellora has to repair it in memory to read it, a bar at the top of the page says so. The file on disk is not changed: Vellora does not edit or save documents yet.

## Find your way around

- The status bar shows the page ("Page iv (4 / 120)" when the document gives its pages names such as roman numerals), the zoom and the state of the document. Click the page text, or press Ctrl+G, to go to a page by number or by its label.
- The sidebar has page thumbnails (F4 shows and hides it), the outline of the document's bookmarks (Ctrl+Shift+O), the search results (Ctrl+Shift+F) and the Info panel (Ctrl+I).
- Everything in the menus is also in the **command palette**: press Ctrl+Shift+P, type part of a name and press Enter.

## Where next

- [Viewing a document](viewing.md): zoom, layouts, rotation, links.
- [Searching](search.md) and [selecting and copying text](text-selection.md).
- [Printing](printing.md).
- All the [keyboard shortcuts](shortcuts.md).
