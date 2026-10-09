// Ends a process the way a crash would, from outside (tests of engine recovery).
#pragma once

#include <QProcess>
#include <QString>
#ifdef Q_OS_WIN
#ifndef NOMINMAX
#define NOMINMAX // windows.h must not define min and max macros
#endif
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#else
#include <csignal>
#include <sys/types.h>
#endif

// Whether a process with this id is still running (a process that has exited, been reaped, and
// whose id nobody reused is gone).
inline bool processExists(quint32 pid) {
#ifdef Q_OS_WIN
    HANDLE process = OpenProcess(SYNCHRONIZE, FALSE, pid);
    if (process == nullptr) {
        return false;
    }
    const bool running = WaitForSingleObject(process, 0) == WAIT_TIMEOUT;
    CloseHandle(process);
    return running;
#else
    return ::kill(static_cast<pid_t>(pid), 0) == 0;
#endif
}

inline void killProcess(quint32 pid) {
#ifdef Q_OS_WIN
    QProcess::execute(QStringLiteral("taskkill"),
                      {QStringLiteral("/F"), QStringLiteral("/PID"), QString::number(pid)});
#else
    QProcess::execute(QStringLiteral("kill"), {QStringLiteral("-9"), QString::number(pid)});
#endif
}
