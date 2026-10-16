// Text that came from a file (a title, an address, a file name) must be shown as written. Qt
// reads a tool tip as rich text when it looks like markup, so a tool tip made of such text is
// escaped and kept as written (white space too) before it is set.
#pragma once

#include <QString>

namespace vellora {

inline QString plainToolTip(const QString& text) {
    if (text.isEmpty()) {
        return text;
    }
    return QStringLiteral("<p style='white-space:pre'>%1</p>").arg(text.toHtmlEscaped());
}

} // namespace vellora
