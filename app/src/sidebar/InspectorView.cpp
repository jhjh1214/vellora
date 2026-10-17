#include "sidebar/InspectorView.h"

#include "PlainText.h"

#include <QCoreApplication>
#include <QFont>
#include <QHeaderView>
#include <QJsonArray>
#include <QJsonDocument>
#include <QLabel>
#include <QLocale>
#include <QTreeWidget>
#include <QTreeWidgetItemIterator>
#include <QVBoxLayout>

namespace vellora {

namespace {

// Every row has a key in `Qt::UserRole`, for tests and for the accessibility tree:
//   section.properties, section.security, section.revisions, section.repairs
//   file_size, version, pages, page_size.<i>, page_sizes.more, info.<field>, tagged, linearized
//   encryption, encryption.unlocked_with, permissions.<name>, permissions.mask
//   feature.<name>, feature.none, feature.incomplete
//   uris (children uris.<i>, uris.more), embedded_files (children embedded_files.<i>, ...)
//   revisions (children revision.<n>), repaired (children repair.<i>), problems (problem.<i>)
constexpr int kKeyRole = Qt::UserRole;

struct FeatureText {
    const char* key;
    const char* title;
    const char* explanation;
};

// The risky or notable features, in the order the report lists them, each with what it means for
// someone who is not a PDF expert. Vellora runs none of them.
const FeatureText kFeatures[] = {
    {"open_action_javascript",
     QT_TRANSLATE_NOOP("vellora::InspectorView", "Runs a script when the file is opened"),
     QT_TRANSLATE_NOOP(
         "vellora::InspectorView",
         "JavaScript that other PDF viewers may run as soon as the document opens. Vellora "
         "never runs PDF JavaScript.")},
    {"javascript_actions",
     QT_TRANSLATE_NOOP("vellora::InspectorView", "Contains JavaScript actions"),
     QT_TRANSLATE_NOOP(
         "vellora::InspectorView",
         "Scripts attached to links, buttons, form fields or bookmarks. Vellora never runs "
         "PDF JavaScript.")},
    {"names_javascript", QT_TRANSLATE_NOOP("vellora::InspectorView", "Has document-level scripts"),
     QT_TRANSLATE_NOOP(
         "vellora::InspectorView",
         "Scripts stored with the document itself, which some viewers run when it opens.")},
    {"additional_actions",
     QT_TRANSLATE_NOOP("vellora::InspectorView", "Reacts to events (open, close, print, focus)"),
     QT_TRANSLATE_NOOP(
         "vellora::InspectorView",
         "Actions that fire when the document is opened, closed or printed, or when a form "
         "field is entered. Vellora does not carry them out.")},
    {"launch_actions",
     QT_TRANSLATE_NOOP("vellora::InspectorView", "Can start a program or open a file"),
     QT_TRANSLATE_NOOP(
         "vellora::InspectorView",
         "A Launch action asks a viewer to start another program or open a file on your "
         "computer. Vellora never follows it.")},
    {"uri_actions", QT_TRANSLATE_NOOP("vellora::InspectorView", "Links to addresses"),
     QT_TRANSLATE_NOOP(
         "vellora::InspectorView",
         "Links that open a web or mail address. Vellora asks before opening one; other "
         "viewers may not. The addresses are listed below.")},
    {"submit_form_actions",
     QT_TRANSLATE_NOOP("vellora::InspectorView", "Sends form data to an address"),
     QT_TRANSLATE_NOOP("vellora::InspectorView",
                       "A form that can send what you typed to a server.")},
    {"goto_remote_actions", QT_TRANSLATE_NOOP("vellora::InspectorView", "Opens other PDF files"),
     QT_TRANSLATE_NOOP("vellora::InspectorView",
                       "A link that opens another PDF document. Vellora does not follow it.")},
    {"embedded_files", QT_TRANSLATE_NOOP("vellora::InspectorView", "Has embedded files"),
     QT_TRANSLATE_NOOP(
         "vellora::InspectorView",
         "Files attached inside the PDF. Vellora never opens them; their names are listed "
         "below.")},
    {"acroform", QT_TRANSLATE_NOOP("vellora::InspectorView", "Has an interactive form"),
     QT_TRANSLATE_NOOP("vellora::InspectorView",
                       "The document has form fields that can be filled in.")},
    {"xfa", QT_TRANSLATE_NOOP("vellora::InspectorView", "Has an XFA form"),
     QT_TRANSLATE_NOOP("vellora::InspectorView",
                       "A form in an older XML-based format, which Vellora does not support.")},
    {"optional_content", QT_TRANSLATE_NOOP("vellora::InspectorView", "Has layers"),
     QT_TRANSLATE_NOOP("vellora::InspectorView",
                       "Content that can be shown or hidden (optional content groups).")},
    {"signature_fields", QT_TRANSLATE_NOOP("vellora::InspectorView", "Has signature fields"),
     QT_TRANSLATE_NOOP(
         "vellora::InspectorView",
         "Digital signatures are present. Vellora lists them; it does not verify them yet.")},
};

struct PermissionText {
    const char* key;
    const char* title;
};

const PermissionText kPermissions[] = {
    {"print", QT_TRANSLATE_NOOP("vellora::InspectorView", "Print")},
    {"print_high_quality", QT_TRANSLATE_NOOP("vellora::InspectorView", "Print at full quality")},
    {"copy", QT_TRANSLATE_NOOP("vellora::InspectorView", "Copy text and images")},
    {"modify", QT_TRANSLATE_NOOP("vellora::InspectorView", "Change the document")},
    {"annotate", QT_TRANSLATE_NOOP("vellora::InspectorView", "Add notes and comments")},
    {"fill_forms", QT_TRANSLATE_NOOP("vellora::InspectorView", "Fill in forms")},
    {"accessibility", QT_TRANSLATE_NOOP("vellora::InspectorView", "Read aloud for accessibility")},
    {"assemble", QT_TRANSLATE_NOOP("vellora::InspectorView", "Insert, rotate and delete pages")},
};

QString yesNo(bool value) {
    return value ? QObject::tr("yes") : QObject::tr("no");
}

QTreeWidgetItem* add(QTreeWidgetItem* parent, const QString& key, const QString& label,
                     const QString& value = QString(), const QString& tip = QString()) {
    auto* item = new QTreeWidgetItem(parent);
    item->setText(0, label);
    item->setText(1, value);
    item->setData(0, kKeyRole, key);
    // The whole text for a value cut short by the width of the panel. What the file supplied is
    // escaped (Qt would read markup in a tool tip); an explanation is ours, in words.
    if (!tip.isEmpty()) {
        item->setToolTip(0, tip);
        item->setToolTip(1, tip);
    } else {
        item->setToolTip(
            0, plainToolTip(value.isEmpty() ? label : label + QLatin1String(": ") + value));
        item->setToolTip(1, plainToolTip(value));
    }
    return item;
}

QTreeWidgetItem* addSection(QTreeWidget* tree, const QString& key, const QString& title) {
    auto* item = new QTreeWidgetItem(tree);
    item->setText(0, title);
    item->setData(0, kKeyRole, key);
    QFont font = item->font(0);
    font.setBold(true);
    item->setFont(0, font);
    return item;
}

// The entries of a `StringList` object as a parent row with one child each.
void addList(QTreeWidgetItem* parent, const QString& key, const QString& title,
             const QJsonValue& value) {
    if (!value.isObject()) {
        return;
    }
    const QJsonObject list = value.toObject();
    const qint64 total = list.value(QLatin1String("total")).toInteger();
    if (total == 0) {
        return;
    }
    auto* head = add(parent, key, title, QString::number(total));
    const QJsonArray items = list.value(QLatin1String("items")).toArray();
    for (qsizetype i = 0; i < items.size(); ++i) {
        const QString text = items.at(i).toString();
        add(head, QStringLiteral("%1.%2").arg(key).arg(i), text);
    }
    if (list.value(QLatin1String("truncated")).toBool()) {
        add(head, key + QLatin1String(".more"),
            QObject::tr("… and %1 more").arg(total - items.size()));
    }
}

} // namespace

InspectorView::InspectorView(EngineSession* session, QWidget* parent)
    : QWidget(parent), m_session(session) {
    setObjectName(QStringLiteral("inspectorView"));
    m_tree = new QTreeWidget(this);
    m_tree->setObjectName(QStringLiteral("inspectorTree"));
    m_tree->setColumnCount(2);
    m_tree->setHeaderHidden(true);
    m_tree->setRootIsDecorated(true);
    m_tree->setUniformRowHeights(true);
    m_tree->setTextElideMode(Qt::ElideMiddle);
    m_tree->setAlternatingRowColors(true);
    m_tree->setSelectionMode(QAbstractItemView::SingleSelection);
    m_tree->setAccessibleName(tr("Document inspector"));
    m_tree->header()->setSectionResizeMode(0, QHeaderView::ResizeToContents);
    m_tree->header()->setStretchLastSection(true);

    m_message = new QLabel(this);
    m_message->setObjectName(QStringLiteral("inspectorMessage"));
    m_message->setTextFormat(Qt::PlainText);
    m_message->setWordWrap(true);
    m_message->setMargin(8);
    m_message->hide();

    auto* layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(0);
    layout->addWidget(m_message);
    layout->addWidget(m_tree, 1);
    setFocusProxy(m_tree);

    connect(m_session, &EngineSession::summaryReady, this, &InspectorView::onSummary);
    connect(m_session, &EngineSession::requestFailed, this, &InspectorView::onRequestFailed);
    connect(m_session, &EngineSession::engineCrashed, this, &InspectorView::onEngineCrashed);
    connect(m_session, &EngineSession::engineRestarted, this, &InspectorView::onEngineRestarted);
}

void InspectorView::reset() {
    m_request = 0;
    m_askAgain = false;
    m_loaded = false;
    m_json.clear();
    m_report = QJsonObject();
    m_tree->clear();
    m_message->hide();
}

void InspectorView::refresh() {
    if (m_loaded || m_request != 0 || !m_session->isOpen()) {
        return;
    }
    m_request = m_session->requestSummary();
    if (m_request == 0) {
        showMessage(tr("The report cannot be made now: the engine is not available."));
    } else {
        showMessage(tr("Reading the document…"));
    }
}

void InspectorView::reload() {
    reset();
    refresh();
}

void InspectorView::showMessage(const QString& text) {
    m_message->setText(text);
    m_message->setVisible(!text.isEmpty());
}

void InspectorView::onRequestFailed(quint64 request, const QString& message) {
    if (request == 0 || request != m_request) {
        return;
    }
    m_request = 0;
    showMessage(tr("The report could not be made: %1").arg(message));
    emit loadFailed(message);
}

void InspectorView::onEngineCrashed(const QString&, bool willRestart, const QList<quint64>& lost) {
    if (m_request != 0 && lost.contains(m_request)) {
        m_request = 0;
        m_askAgain = willRestart;
        if (!willRestart) {
            showMessage(tr("The report could not be made: the engine stopped."));
            emit loadFailed(tr("the engine stopped"));
        }
    }
}

void InspectorView::onEngineRestarted() {
    if (m_askAgain) {
        m_askAgain = false;
        refresh();
    }
}

void InspectorView::onSummary(quint64 request, const QString& json) {
    if (request == 0 || request != m_request) {
        return;
    }
    m_request = 0;
    QJsonParseError error;
    const QJsonDocument document = QJsonDocument::fromJson(json.toUtf8(), &error);
    if (error.error != QJsonParseError::NoError || !document.isObject()) {
        showMessage(tr("The report could not be read: %1").arg(error.errorString()));
        emit loadFailed(error.errorString());
        return;
    }
    m_json = json;
    m_report = document.object();
    m_loaded = true;
    show(m_report);
    showMessage(QString());
    emit loaded();
}

QTreeWidgetItem* InspectorView::find(const QString& key) const {
    for (QTreeWidgetItemIterator it(m_tree); *it != nullptr; ++it) {
        if ((*it)->data(0, kKeyRole).toString() == key) {
            return *it;
        }
    }
    return nullptr;
}

QString InspectorView::valueOf(const QString& key) const {
    const QTreeWidgetItem* item = find(key);
    return item != nullptr ? item->text(1) : QString();
}

QString InspectorView::explanation(const QString& feature) {
    for (const FeatureText& text : kFeatures) {
        if (feature == QLatin1String(text.key)) {
            return QCoreApplication::translate("vellora::InspectorView", text.explanation);
        }
    }
    return {};
}

QStringList InspectorView::shownFeatures() const {
    QStringList shown;
    for (const FeatureText& text : kFeatures) {
        if (find(QStringLiteral("feature.") + QLatin1String(text.key)) != nullptr) {
            shown.append(QLatin1String(text.key));
        }
    }
    return shown;
}

void InspectorView::show(const QJsonObject& r) {
    m_tree->clear();
    const auto text = [&](const char* key) { return r.value(QLatin1String(key)).toString(); };
    const bool locked = r.value(QLatin1String("locked")).toBool();
    const QString unknown = locked ? tr("unknown (locked)") : tr("unknown");

    // ---- properties ----
    auto* properties = addSection(m_tree, QStringLiteral("section.properties"), tr("Properties"));
    const qint64 size = r.value(QLatin1String("file_size")).toInteger();
    add(properties, QStringLiteral("file_size"), tr("File size"),
        tr("%1 (%2 bytes)").arg(QLocale().formattedDataSize(size)).arg(size));
    add(properties, QStringLiteral("version"), tr("PDF version"),
        r.value(QLatin1String("version")).isString() ? text("version") : tr("unknown"));
    add(properties, QStringLiteral("pages"), tr("Pages"),
        r.value(QLatin1String("pages")).isNull()
            ? unknown
            : QString::number(r.value(QLatin1String("pages")).toInteger()));
    const QJsonArray sizes = r.value(QLatin1String("page_sizes")).toArray();
    for (qsizetype i = 0; i < sizes.size(); ++i) {
        const QJsonObject s = sizes.at(i).toObject();
        const qint64 pages = s.value(QLatin1String("pages")).toInteger();
        add(properties, QStringLiteral("page_size.%1").arg(i), tr("Page size"),
            tr("%1 × %2 pt (%n page(s))", nullptr, static_cast<int>(pages))
                .arg(s.value(QLatin1String("width")).toDouble())
                .arg(s.value(QLatin1String("height")).toDouble()));
    }
    if (r.value(QLatin1String("page_sizes_truncated")).toBool()) {
        add(properties, QStringLiteral("page_sizes.more"), tr("Page size"), tr("and more"));
    }
    const QJsonObject info = r.value(QLatin1String("info")).toObject();
    const std::pair<const char*, const char*> infoFields[] = {
        {"title", QT_TRANSLATE_NOOP("vellora::InspectorView", "Title")},
        {"author", QT_TR_NOOP("Author")},
        {"subject", QT_TRANSLATE_NOOP("vellora::InspectorView", "Subject")},
        {"keywords", QT_TR_NOOP("Keywords")},
        {"creator", QT_TRANSLATE_NOOP("vellora::InspectorView", "Created with")},
        {"producer", QT_TR_NOOP("PDF produced by")},
        {"creation_date", QT_TRANSLATE_NOOP("vellora::InspectorView", "Created")},
        {"modification_date", QT_TR_NOOP("Modified")},
    };
    for (const auto& [key, title] : infoFields) {
        const QJsonValue value = info.value(QLatin1String(key));
        if (value.isString()) {
            add(properties, QStringLiteral("info.") + QLatin1String(key),
                QCoreApplication::translate("vellora::InspectorView", title), value.toString());
        }
    }
    if (r.value(QLatin1String("tagged")).isBool()) {
        add(properties, QStringLiteral("tagged"), tr("Tagged for accessibility"),
            yesNo(r.value(QLatin1String("tagged")).toBool()));
    }
    add(properties, QStringLiteral("linearized"), tr("Optimised for fast web view"),
        yesNo(r.value(QLatin1String("linearized")).toBool()));

    // ---- security ----
    auto* security = addSection(m_tree, QStringLiteral("section.security"), tr("Security"));
    const QJsonValue encryption = r.value(QLatin1String("encryption"));
    if (!encryption.isObject()) {
        add(security, QStringLiteral("encryption"), tr("Encryption"), tr("none"));
    } else {
        const QJsonObject e = encryption.toObject();
        add(security, QStringLiteral("encryption"), tr("Encryption"),
            tr("%1 handler, version %2 revision %3, %4-bit key, streams %5, strings %6")
                .arg(e.value(QLatin1String("handler")).toString())
                .arg(e.value(QLatin1String("version")).toInt())
                .arg(e.value(QLatin1String("revision")).toInt())
                .arg(e.value(QLatin1String("key_bits")).toInt())
                .arg(e.value(QLatin1String("stream_method")).toString(),
                     e.value(QLatin1String("string_method")).toString()));
        add(security, QStringLiteral("encryption.unlocked_with"), tr("Opened with"),
            e.value(QLatin1String("unlocked_with")).isString()
                ? tr("the %1 password").arg(e.value(QLatin1String("unlocked_with")).toString())
                : tr("no password (locked)"));
        const QJsonObject allowed = e.value(QLatin1String("allowed")).toObject();
        auto* permissions = add(security, QStringLiteral("permissions"), tr("The author allows"));
        for (const PermissionText& permission : kPermissions) {
            add(permissions, QStringLiteral("permissions.") + QLatin1String(permission.key),
                QCoreApplication::translate("vellora::InspectorView", permission.title),
                yesNo(allowed.value(QLatin1String(permission.key)).toBool()),
                tr("A viewer is asked to follow these; they are not a lock."));
        }
        add(permissions, QStringLiteral("permissions.mask"), tr("Permission bits"),
            QString::number(e.value(QLatin1String("permissions")).toInt()));
    }

    const QJsonObject features = r.value(QLatin1String("features")).toObject();
    int found = 0;
    if (!features.isEmpty()) {
        for (const FeatureText& feature : kFeatures) {
            if (!features.value(QLatin1String(feature.key)).toBool()) {
                continue;
            }
            ++found;
            add(security, QStringLiteral("feature.") + QLatin1String(feature.key),
                QCoreApplication::translate("vellora::InspectorView", feature.title), tr("found"),
                QCoreApplication::translate("vellora::InspectorView", feature.explanation));
        }
        if (found == 0) {
            add(security, QStringLiteral("feature.none"), tr("Risky features"), tr("none found"));
        }
        if (!features.value(QLatin1String("complete")).toBool()) {
            add(security, QStringLiteral("feature.incomplete"), tr("Scan"),
                tr("incomplete: a feature that is not listed may have been missed"));
        }
    } else {
        add(security, QStringLiteral("feature.none"), tr("Risky features"), unknown);
    }
    addList(security, QStringLiteral("uris"), tr("Addresses in links"),
            r.value(QLatin1String("uris")));
    addList(security, QStringLiteral("embedded_files"), tr("Embedded files"),
            r.value(QLatin1String("embedded_files")));

    // ---- revisions ----
    const QJsonArray ranges = r.value(QLatin1String("revision_ranges")).toArray();
    auto* revisions = addSection(m_tree, QStringLiteral("section.revisions"), tr("Revisions"));
    add(revisions, QStringLiteral("revisions"), tr("Saved versions in the file"),
        QString::number(r.value(QLatin1String("revisions")).toInteger()));
    for (const QJsonValue& value : ranges) {
        const QJsonObject range = value.toObject();
        add(revisions,
            QStringLiteral("revision.%1").arg(range.value(QLatin1String("number")).toInteger()),
            tr("Version %1").arg(range.value(QLatin1String("number")).toInteger()),
            tr("bytes %1 to %2")
                .arg(range.value(QLatin1String("start")).toInteger())
                .arg(range.value(QLatin1String("end")).toInteger()));
    }

    // ---- repairs ----
    auto* repairs = addSection(m_tree, QStringLiteral("section.repairs"), tr("Repairs"));
    const bool repaired = r.value(QLatin1String("repaired")).toBool();
    auto* head =
        add(repairs, QStringLiteral("repaired"), tr("The file needed repair"), yesNo(repaired));
    const QJsonArray reasons = r.value(QLatin1String("repair_reasons")).toArray();
    for (qsizetype i = 0; i < reasons.size(); ++i) {
        add(head, QStringLiteral("repair.%1").arg(i), reasons.at(i).toString());
    }
    const QJsonArray problems = r.value(QLatin1String("problems")).toArray();
    if (!problems.isEmpty()) {
        auto* list = add(repairs, QStringLiteral("problems"), tr("Problems while reading"),
                         QString::number(problems.size()));
        for (qsizetype i = 0; i < problems.size(); ++i) {
            add(list, QStringLiteral("problem.%1").arg(i), problems.at(i).toString());
        }
    }

    m_tree->expandAll();
}

} // namespace vellora
