#include "navigation/NavigationHistory.h"

namespace vellora {

void NavigationHistory::push(QVector<Place>& stack, const Place& place) {
    // The same place twice in a row is one place (a jump that went nowhere).
    if (!stack.isEmpty() && stack.last() == place) {
        return;
    }
    stack.append(place);
    if (stack.size() > kMaxEntries) {
        stack.removeFirst();
    }
}

void NavigationHistory::recordJump(const Place& from) {
    push(m_back, from);
    m_forward.clear();
    emit changed();
}

std::optional<NavigationHistory::Place> NavigationHistory::back(const Place& current) {
    if (m_back.isEmpty()) {
        return std::nullopt;
    }
    const Place target = m_back.takeLast();
    push(m_forward, current);
    emit changed();
    return target;
}

std::optional<NavigationHistory::Place> NavigationHistory::forward(const Place& current) {
    if (m_forward.isEmpty()) {
        return std::nullopt;
    }
    const Place target = m_forward.takeLast();
    push(m_back, current);
    emit changed();
    return target;
}

void NavigationHistory::clear() {
    if (m_back.isEmpty() && m_forward.isEmpty()) {
        return;
    }
    m_back.clear();
    m_forward.clear();
    emit changed();
}

} // namespace vellora
