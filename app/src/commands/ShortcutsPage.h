// The keyboard shortcuts page of the user manual (docs/user/shortcuts.md), written from the command
// registry so that the manual cannot drift from the application. CI runs `vellora
// --export-shortcuts` and fails if the committed page differs.
#pragma once

#include "commands/CommandRegistry.h"

#include <QString>

namespace vellora {

// The page as Markdown: the commands that have a shortcut, grouped by the part of their id before
// the first dot, in registration order. Shortcuts are written in Qt's portable text ("Ctrl+O",
// never the native "⌘O").
QString shortcutsMarkdown(const CommandRegistry& registry);

} // namespace vellora
