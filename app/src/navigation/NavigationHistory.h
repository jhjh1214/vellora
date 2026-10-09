// Back and forward through the places the reader jumped from: an outline item, Go to Page, a
// thumbnail, later a link. Only jumps are recorded; scrolling, page turns and zooming are not, so
// Back returns to where the reader was before the last jump, however far they had scrolled.
//
// Like a browser's history: a new jump after going back forgets what was ahead.
#pragma once

#include <QObject>
#include <QVector>
#include <optional>

namespace vellora {

class NavigationHistory : public QObject {
    Q_OBJECT

public:
    // The most places kept in either direction; the oldest is dropped beyond it.
    static constexpr int kMaxEntries = 100;

    // A place in the document: the point at the top edge of the window, and the zoom.
    struct Place {
        quint32 page = 0;
        double offsetPoints = 0.0; // down from the top of `page`
        double zoom = 1.0;
        friend bool operator==(const Place&, const Place&) = default;
    };

    explicit NavigationHistory(QObject* parent = nullptr) : QObject(parent) {}

    // The reader is about to jump away from `from`.
    void recordJump(const Place& from);
    // Where Back goes, given where the reader is now (which Forward then returns to); empty when
    // there is nothing to go back to. Likewise Forward.
    std::optional<Place> back(const Place& current);
    std::optional<Place> forward(const Place& current);

    bool canGoBack() const { return !m_back.isEmpty(); }
    bool canGoForward() const { return !m_forward.isEmpty(); }
    int backCount() const { return static_cast<int>(m_back.size()); }
    int forwardCount() const { return static_cast<int>(m_forward.size()); }
    // Forgets everything (another document was opened).
    void clear();

signals:
    void changed();

private:
    static void push(QVector<Place>& stack, const Place& place);

    QVector<Place> m_back;
    QVector<Place> m_forward;
};

} // namespace vellora
