// The Inspector: what is in the document, shown from the report the engine makes (the same JSON
// `vellora inspect --json` prints). Properties, security (encryption, permissions, risky features
// with a plain explanation of each, addresses and embedded file names), revisions with their byte
// ranges, and repairs. The shell never reads the document: it asks the engine and shows what comes
// back, as plain text, since every string in it came from the file.
#pragma once

#include "bridge/EngineSession.h"

#include <QByteArray>
#include <QJsonObject>
#include <QString>
#include <QWidget>

class QLabel;
class QTreeWidget;
class QTreeWidgetItem;

namespace vellora {

class InspectorView : public QWidget {
    Q_OBJECT

public:
    // `session` is not owned and must outlive the view.
    explicit InspectorView(EngineSession* session, QWidget* parent = nullptr);

    // Forgets the previous document's report; call before the session opens another file.
    void reset();
    // Asks the engine for the report unless it is here or on its way. `loaded` follows.
    void refresh();
    // Asks again (the file may have changed).
    void reload();

    bool isLoaded() const { return m_loaded; }
    bool isLoading() const { return m_request != 0; }
    // The report as the engine sent it: the text `vellora inspect --json` prints, without the
    // final newline. Empty until loaded.
    QString json() const { return m_json; }
    const QJsonObject& report() const { return m_report; }

    QTreeWidget& tree() { return *m_tree; }
    // The row for `key` (see InspectorView.cpp for the keys), or null.
    QTreeWidgetItem* find(const QString& key) const;
    // What the row shows in its value column; empty if there is no such row.
    QString valueOf(const QString& key) const;
    // The plain-language explanation of a risky feature, as shown in its row's tool tip.
    static QString explanation(const QString& feature);
    // The keys of the feature rows shown for the report (the features found).
    QStringList shownFeatures() const;

signals:
    // The report arrived and is shown.
    void loaded();
    // It could not be had (`message` says why); the view says so too.
    void loadFailed(const QString& message);

private slots:
    void onSummary(quint64 request, const QString& json);
    void onRequestFailed(quint64 request, const QString& message);
    void onEngineCrashed(const QString& how, bool willRestart, const QList<quint64>& lost);
    void onEngineRestarted();

private:
    void show(const QJsonObject& report);
    void showMessage(const QString& text);

    EngineSession* m_session;
    QTreeWidget* m_tree;
    QLabel* m_message;
    quint64 m_request = 0;
    // The engine died with the request in flight: ask again once it is back.
    bool m_askAgain = false;
    bool m_loaded = false;
    QString m_json;
    QJsonObject m_report;
};

} // namespace vellora
