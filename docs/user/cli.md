# The `vellora` command line

```
vellora <COMMAND> [OPTIONS]
vellora --help | --version
```

**Exit codes** (every command):

| Code | Meaning |
|---|---|
| 0 | The command did what was asked. |
| 1 | The operation failed: the file cannot be read, is not a PDF that Vellora can open, or the password is wrong. A message starting with `vellora: ` is written to stderr and nothing to stdout. |
| 2 | Usage error: unknown command, missing or unknown argument. |
| 3 | Only `vellora security`: the file has risky features. |

Commands never run or open anything found inside a document: no JavaScript, no launch actions, no attachments, no network access.

## `vellora inspect`

```
vellora inspect <FILE> [--json] [--password <PASSWORD>]
```

Reads the file and reports what it contains. The whole file is read into memory.

| Option | Meaning |
|---|---|
| `--json` | Print the report as JSON (schema below) instead of a table. |
| `--password <PASSWORD>` | Password for an encrypted file, user or owner. It is tried as UTF-8 and, if it fits in Latin-1, as Latin-1 bytes; no Unicode normalisation is applied. A wrong password is exit code 1, even for an encrypted file that opens with the empty password; an unencrypted file ignores the option. The password is visible in the process list. |

An encrypted file that needs a password and was not given one still gets a report (exit code 0): the encryption is described, `locked` is `true`, and `pages` and `features` are `null` because nothing can be read. A note about `--password` is written to stderr.

### Table output

```
File          report.pdf
Size          607 bytes
PDF version   1.7
Pages         1
Objects       4
Revisions     1
Repaired      no
Encryption    none

Features
  JavaScript on open              yes
  ...
```

The table is for people; its layout may change. Scripts should use `--json`.

### JSON schema

Field names and meanings are stable within a major version. New fields may be added, so readers must ignore fields they do not know.

| Field | Type | Meaning |
|---|---|---|
| `file_size` | integer | Size of the file in bytes. |
| `version` | string or `null` | Version in the `%PDF-M.m` header, e.g. `"1.7"`; `null` if the header has none. |
| `objects` | integer | Objects the cross-reference lists as in use, compressed ones included. Not checked against the file. |
| `revisions` | integer | Revisions in the file's own incremental history; 1 for a file never updated, and for one whose cross-reference had to be rebuilt. |
| `repaired` | boolean | The file had to be repaired to be read. |
| `repair_reasons` | array of strings | One line per reason; empty when `repaired` is `false`. Free text, for people. |
| `encryption` | object or `null` | `null` for an unencrypted document. See below. |
| `locked` | boolean | Encrypted and no password was accepted. |
| `pages` | integer or `null` | Pages found by walking the page tree (`/Count` is not trusted); `null` when `locked`. |
| `features` | object or `null` | See below; `null` when `locked`. |
| `problems` | array of strings | Things that went wrong while reading, so some numbers may be too low (at most 16). Free text. |
| `info` | object or `null` | The document information dictionary as text: `title`, `author`, `subject`, `keywords`, `creator`, `producer`, `creation_date`, `modification_date`, each a string or `null` (each cut to 1,024 characters). Dates are given as the file has them (`D:YYYYMMDDHHmmSS...`). `null` when the file has no information dictionary and when `locked`. |
| `tagged` | boolean or `null` | The catalog says the document is tagged (`/MarkInfo` `/Marked true`); `null` when `locked`. |
| `linearized` | boolean | The first object of the file is a linearization dictionary. Only that dictionary is looked for; the hint tables are not checked. |
| `page_sizes` | array or `null` | The sizes of the pages as displayed (the crop box if the page has one, else the media box, turned by `/Rotate`): objects with `width` and `height` in points to two decimals and `pages`, the number of pages of that size, most pages first. At most 32 sizes; `null` when `locked`. |
| `page_sizes_truncated` | boolean | There are more distinct sizes than `page_sizes` lists. |
| `revision_ranges` | array | One object per revision, oldest first: `number` (1 for the oldest), `start` and `end` (byte offsets; `end` is one past the `%%EOF` marker). They follow one another without a gap. |
| `uris` | object or `null` | The addresses of the `URI` actions: `items` (strings, the first 100, each cut to 1,024 characters), `total` (how many were met) and `truncated`. `null` when `locked`. Nothing is opened. |
| `embedded_files` | object or `null` | The names of the embedded files (the `/EmbeddedFiles` name tree and file attachment annotations), as `uris`. |

`encryption` (Standard Security Handler):

| Field | Type | Meaning |
|---|---|---|
| `handler` | string | Always `"Standard"`; other handlers are not supported and make the command fail. |
| `version` | integer | `/V`, the algorithm version. |
| `revision` | integer | `/R`, 2 to 6. |
| `key_bits` | integer | Length of the file encryption key in bits. |
| `permissions` | integer | `/P` as a signed 32-bit integer. |
| `allowed` | object | `permissions` decoded into booleans: `print`, `modify`, `copy`, `annotate`, `fill_forms`, `accessibility`, `assemble`, `print_high_quality`. These bind a conforming reader; they are not a security boundary. |
| `stream_method` | string | `"identity"`, `"rc4"`, `"aes-128"` or `"aes-256"`. |
| `string_method` | string | As `stream_method`, for strings. |
| `encrypt_metadata` | boolean | Whether the metadata stream is encrypted. |
| `unlocked_with` | string or `null` | `"user"` (the empty password counts) or `"owner"`; `null` when locked. |

`features` — each flag is `true` when the feature was found in the places the scan looks:
the catalog, every page and its annotations, the form fields, and the bookmarks.
Actions are followed along `/Next` chains. Only dictionaries are read; scripts are never decoded.

| Field | Meaning |
|---|---|
| `complete` | The scan read everything. `false` means it hit its work limit or could not read some objects (see `problems`), so a `false` flag below may be a miss. |
| `open_action_javascript` | The catalog's `/OpenAction` is, or chains to, a JavaScript action: it runs when the file is opened. |
| `javascript_actions` | A JavaScript action anywhere in the scanned places. |
| `names_javascript` | The catalog has `/Names` `/JavaScript` (document-level scripts). |
| `additional_actions` | An `/AA` dictionary on the catalog, a page, an annotation or a form field. |
| `launch_actions` | A `Launch` action. |
| `uri_actions` | A `URI` action. |
| `submit_form_actions` | A `SubmitForm` action. |
| `goto_remote_actions` | A `GoToR` action (opens another PDF file). |
| `embedded_files` | `/Names` `/EmbeddedFiles`, or a file attachment annotation. |
| `acroform` | The catalog has an `/AcroForm`. |
| `xfa` | The `/AcroForm` has `/XFA`. |
| `optional_content` | The catalog has `/OCProperties` (layers). |
| `signature_fields` | A field with type `/Sig`, including one that inherits the type from its parent. Signatures are listed, not verified. |

The additions after `problems` (task 18) were made without changing any earlier field.

Example (`vellora inspect --json`, abbreviated):

```json
{
  "file_size": 329,
  "version": "1.7",
  "objects": 3,
  "revisions": 1,
  "repaired": false,
  "repair_reasons": [],
  "encryption": null,
  "locked": false,
  "pages": 1,
  "features": { "complete": true, "open_action_javascript": false, "...": false },
  "problems": []
}
```

## `vellora security`

```
vellora security <FILE> [--json] [--password <PASSWORD>]
```

Answers one question: does this file have features that make it risky to open? It runs in the process of the command, reads the file as `inspect` does, and opens or runs nothing.

**Exit codes:** 0 no risky features were found; **3** risky features were found; 1 the file cannot be read or assessed (an encrypted file without its password cannot be: pass `--password`); 2 usage.

The features counted as risky are: a script that runs when the file opens, JavaScript actions, document-level scripts, event actions (`/AA`), launch actions, form submission, links that open other PDF files, embedded files and XFA forms. Links to addresses, forms, layers and signature fields are listed as *also present* but do not make a file risky. A scan that could not read everything (`"complete": false`) may have missed a feature: the text says so, and the exit code is still the one for what was found.

The text output gives the verdict, each finding with a sentence on what it means, the addresses in links and the names of embedded files (text from the file is shown on one line, with control characters and invisible formatting replaced by spaces).

`--json` prints:

| Field | Type | Meaning |
|---|---|---|
| `risky` | boolean | At least one finding. |
| `complete` | boolean | The scan read everything; when `false`, a feature not listed may have been missed. |
| `findings` | array | Objects with `id` (the name of the flag in `inspect`'s `features`), `title` and `explanation`. |
| `notes` | array | Objects with `id` and `title`: features that are present and not risky by themselves. |
| `uris` | object | The addresses of links: `items` (at most 100), `total`, `truncated`. |
| `embedded_files` | object | The names of embedded files, as `uris`. |
| `encrypted` | boolean | The file is encrypted (and was opened with the password given). |
| `repaired` | boolean | The file had to be repaired to be read, so the scan may be incomplete. |
| `problems` | array of strings | Things that went wrong while reading. |

## `vellora render`

```
vellora render <FILE> -o <PATH> [--pages <PAGES>] [--dpi <N>] [--password <PASSWORD>]
```

Draws pages as PNG files with **the engine**: the same sandboxed process the desktop application uses, started for the run. Nothing in the file is run.

| Option | Meaning |
|---|---|
| `--pages <PAGES>` | A page specification (below). The default is the first page. |
| `--dpi <N>` | Resolution in dots per inch, 1 to 4,608; the default is 150. A page that would be more than 134 million pixels is refused: lower the resolution. |
| `-o`, `--output <PATH>` | A folder (made if it is not there), in which the pages are written as `page-N.png`, numbered by page and padded to the width of the page count; or, for exactly one page, a file name ending in `.png`. |
| `--password <PASSWORD>` | As for `inspect`. |

The files written are listed on stdout, one per line. The image is RGB, 8 bits, drawn the way the screen draws a page (annotations that need no interaction included).

The engine is looked for next to the `vellora` program (set `VELLORA_ENGINE` to use another), and PDFium next to the engine (set `VELLORA_PDFIUM_LIB` to use another).

## `vellora text`

```
vellora text <FILE> [--pages <PAGES>] [--password <PASSWORD>]
```

Prints the text of pages in reading order, as the engine extracts it (the order PDFium reads it in, which is the order of the content for most documents and not always the order a person reads columns in). Text in images is not recognised. Line ends are `\n`. Pages are separated by a form feed (U+000C), as `pdftotext` does, with none after the last. `--pages` defaults to all pages; pages are printed in the order given.

## Page specifications

`--pages` takes a list of pieces separated by commas: `N` is one page, `N-M` a range, `N-` runs to the last page and `-M` starts at the first. Pages count from 1. `1-5,8,10-` is pages 1 to 5, 8, and 10 to the end. A page named twice is printed once, where it first came. A page the document does not have, page 0, a range that runs backwards and anything else that is not a page are refused (exit code 1) with a message that says which.
