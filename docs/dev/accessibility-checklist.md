# Accessibility checklist

What the automated tests cannot tell: whether a real screen reader reads Vellora the way the accessibility tree says it should. Run this before a release, and for any change to the canvas, the sidebar or the dialogs. Write the results in the pull request: the tool and its version, the operating system, each step with *pass*, *fail* or *not run*, and what you heard for each failure.

The automated side, which runs in CI: `tst_accessibility` (the tree, page text, links, announcements), `tst_caret` (caret browsing), `tst_commands` (every command has a title and a way to reach it from the keyboard; the palette fits a larger font) and the print tests (every dialog control has an accessible name).

## Before you start

- Use a document with several pages, text, a few links (an internal link and an address) and page labels, and one with a page that is only an image. The synthetic files of `app/tests/` are too plain to judge reading order; a real report or article is better.
- Turn the screen reader on first, then start Vellora (some bridges only attach to applications started after them).
- Note the Qt version (`Help -> About`) and the screen reader version.

## NVDA (Windows 10 or 11)

| # | Step | Expect |
|---|---|---|
| 1 | Open a PDF with File -> Open (Ctrl+O). | NVDA reads the window title; the focus lands in the document. |
| 2 | Press NVDA+Tab on the document area. | It reads the document's name (the file name) and its role (document). |
| 3 | Press F7 (caret browsing on). Press Down. | NVDA reads the line the caret moves to. |
| 4 | Press Ctrl+Right a few times. | It reads one word each time. |
| 5 | Press Right, then Shift+Right five times. | It reads the characters, then says what is selected. |
| 6 | Press Ctrl+End, then Ctrl+Home. | It goes to the end and the start of the text; the page changes are spoken ("Page 12 of 12"). |
| 7 | Press F7 again. | Caret browsing is off; the arrow keys scroll again. |
| 8 | Press F8 (next link). | NVDA reads the link's description ("Go to page 3", or the address). A black and white ring shows round the link. |
| 9 | Press Enter on a link. | An internal link jumps; an address asks first (and the question is read). |
| 10 | Press Page Down until the page changes. | The new page is announced. |
| 11 | In the object navigator (NVDA+Numpad 8 / 2), go into the document. | There is one child per page, named "Page n of N", and each page's text can be read. |
| 12 | Press Ctrl+Shift+P (the command palette) and type "zoom". | The list is read item by item with each shortcut; Enter runs a command; Escape closes it. |
| 13 | Press Ctrl+F, type a word, press F3. | The find bar controls are named; the result count is read; the page is announced when the result is on another page. |
| 14 | Press Ctrl+P. | The print dialogs are navigable: every control has a name, an error in the page range is read. |

## Orca (Linux, GNOME or KDE)

Run the same table with Orca. Differences to expect: the document role is read as "document frame", and the object navigation is Orca's flat review (Numpad 7 / 9 / 8 / 2 / 4 / 6). Set `QT_ACCESSIBILITY=1` if the application does not appear in Orca, and make sure `at-spi2-core` is running.

## VoiceOver (macOS), when a Mac is at hand

The same table with VoiceOver (Control+Option arrows to navigate). Not part of the first release's gate; record what you see.

## System settings

| # | Setting | Expect |
|---|---|---|
| 15 | Windows: turn on a high-contrast theme (Settings -> Accessibility -> Contrast themes) and start Vellora. | Menus, dialogs, the sidebar and the find bar use the theme's colours. The text selection, the caret and the link ring are visible on the page. |
| 16 | Set the system text size to 200% (Windows: Accessibility -> Text size; GNOME: Text Scaling Factor 2.0). Start Vellora. | Menus, the sidebar labels, the find bar and the command palette show their text whole, not cut off. |
| 17 | Turn off animations (Windows: Accessibility -> Visual effects -> Animation effects; GNOME: Enable Animations off; macOS: Reduce motion). | Nothing in Vellora moves by itself: scrolling and page turns land at once. (Vellora has no animation; this step confirms nothing was added.) |

## Keyboard only

| # | Step | Expect |
|---|---|---|
| 18 | Put the mouse out of reach. Open a file, zoom in and out, go to a page, search, follow a link, print, close the tab. | Everything can be done. Alt opens the menus; Ctrl+Shift+P lists every command with its shortcut. |
| 19 | Press Tab and Shift+Tab through the window. | The focus visits the sidebar, the canvas and the find bar in an order that makes sense, and always shows where it is. |

## Recording the result

Copy the table into the pull request and fill in the last column. A failing step is a bug; open an issue or add it to [`docs/backlog.md`](../backlog.md) with the step number. Task 17 of milestone 1 is ticked only when the NVDA and Orca runs are recorded.
