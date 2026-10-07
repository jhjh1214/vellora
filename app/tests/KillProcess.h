// Ends a process the way a crash would, from outside (tests of engine recovery).
#pragma once

#include <QProcess>
#include <QString>

inline void killProcess(quint32 pid) {
#ifdef Q_OS_WIN
    QProcess::execute(QStringLiteral("taskkill"),
                      {QStringLiteral("/F"), QStringLiteral("/PID"), QString::number(pid)});
#else
    QProcess::execute(QStringLiteral("kill"), {QStringLiteral("-9"), QString::number(pid)});
#endif
}
