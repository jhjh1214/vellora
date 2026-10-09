#include "links/LinkActions.h"

#include <QCoreApplication>
#include <QUrl>

namespace vellora {

namespace {

QString tr(const char* text) {
    return QCoreApplication::translate("vellora::LinkActions", text);
}

} // namespace

bool LinkActions::schemeAllowed(const QString& scheme) {
    return scheme == QLatin1String("http") || scheme == QLatin1String("https") ||
           scheme == QLatin1String("mailto");
}

LinkActions::UriCheck LinkActions::checkUri(const QString& uri) {
    UriCheck check;
    if (uri.isEmpty()) {
        check.problem = tr("The link has no address.");
        return check;
    }
    // Whitespace and control characters are not part of an address; showing one would also let
    // a document hide what the address says.
    for (const QChar c : uri) {
        if (c.unicode() < 0x20 || c.unicode() == 0x7F || c.isSpace()) {
            check.problem = tr("The address contains spaces or control characters.");
            return check;
        }
    }
    const QUrl url(uri, QUrl::StrictMode);
    if (!url.isValid()) {
        check.problem = tr("The address is not valid.");
        return check;
    }
    check.scheme = url.scheme().toLower();
    if (!schemeAllowed(check.scheme)) {
        check.problem =
            check.scheme.isEmpty()
                ? tr("The address has no scheme (such as https:), so it cannot be opened.")
                : tr("Vellora opens only web (http, https) and mail (mailto) addresses; this one "
                     "is \"%1:\".")
                      .arg(check.scheme);
        return check;
    }
    if (check.scheme == QLatin1String("mailto")) {
        if (url.path().isEmpty() && !url.hasQuery()) {
            check.problem = tr("The mail address is empty.");
            return check;
        }
    } else {
        check.host = url.host().toLower();
        if (check.host.isEmpty()) {
            check.problem = tr("The web address has no host.");
            return check;
        }
    }
    check.allowed = true;
    return check;
}

QString LinkActions::inertNotice(const QString& kind) {
    return tr("This link would run an action Vellora never runs (%1).").arg(kind);
}

QString LinkActions::describe(const Link& link, const std::function<QString(quint32)>& pageLabel) {
    switch (link.kind) {
    case LinkKind::GoTo:
        return tr("Go to page %1").arg(pageLabel(link.destination.page));
    case LinkKind::Uri:
        return link.text.size() > kMaxToolTipChars
                   ? link.text.left(kMaxToolTipChars) + QStringLiteral("…")
                   : link.text;
    case LinkKind::Named:
        switch (link.named) {
        case NamedKind::NextPage:
            return tr("Go to the next page");
        case NamedKind::PrevPage:
            return tr("Go to the previous page");
        case NamedKind::FirstPage:
            return tr("Go to the first page");
        case NamedKind::LastPage:
            return tr("Go to the last page");
        default:
            return {};
        }
    case LinkKind::Inert:
        return inertNotice(link.text);
    default:
        return {};
    }
}

} // namespace vellora
