// What a link may do, decided in one place. The engine only describes a link (its address, its
// kind of action); the shell follows an internal jump, asks before opening an address, and never
// runs anything else (CLAUDE.md invariant 8). All text here is plain text: addresses and action
// names come from the document.
#pragma once

#include "bridge/Destination.h"

#include <QString>
#include <functional>

namespace vellora {

class LinkActions {
public:
    // The schemes Vellora opens, after asking. Everything else is shown and refused.
    static bool schemeAllowed(const QString& lowerCaseScheme);

    // Whether an address may be opened, and what the question shows.
    struct UriCheck {
        bool allowed = false;
        QString scheme;  // lower case; empty if the address has none
        QString host;    // lower case, `http` and `https` only; what "do not ask again" is for
        QString problem; // why it is refused, for the reader
    };
    static UriCheck checkUri(const QString& uri);

    // "This link would run an action Vellora never runs (Launch)."
    static QString inertNotice(const QString& kind);

    // The tool tip of a link: its target in words. Empty for a link that goes nowhere.
    // `pageLabel` names a page (zero-based) the way the reader knows it.
    static QString describe(const Link& link, const std::function<QString(quint32)>& pageLabel);

    // The longest address a tool tip shows whole; a longer one is cut (the question shows it all).
    static constexpr int kMaxToolTipChars = 200;
};

} // namespace vellora
