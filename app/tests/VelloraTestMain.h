// QTEST_MAIN with `vellora::Application` as the application object, for the tests that need its
// event timing (it is a QApplication otherwise).
#pragma once

#include "diagnostics/Application.h"

#include <QTest>

#define VELLORA_TEST_MAIN(TestObject)                                                              \
    int main(int argc, char** argv) {                                                              \
        vellora::Application app(argc, argv);                                                      \
        app.setAttribute(Qt::AA_Use96Dpi, true);                                                   \
        TestObject tc;                                                                             \
        QTEST_SET_MAIN_SOURCE_PATH                                                                 \
        return QTest::qExec(&tc, argc, argv);                                                      \
    }
