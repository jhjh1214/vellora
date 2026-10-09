// AppSettings: recent files, saved view states and the rest of what is remembered between runs.
#include "settings/AppSettings.h"

#include <QDateTime>
#include <QFile>
#include <QSettings>
#include <QTemporaryDir>
#include <QTest>
#include <limits>

using vellora::AppSettings;

namespace {

QString touch(const QTemporaryDir& dir, const QString& name, const QByteArray& content = "x") {
    const QString path = dir.filePath(name);
    QFile file(path);
    if (!file.open(QIODevice::WriteOnly) || file.write(content) != content.size()) {
        return {};
    }
    return path;
}

} // namespace

class TstSettings : public QObject {
    Q_OBJECT

private slots:
    void nothingIsRememberedWithoutASettingsObject() {
        AppSettings settings;
        settings.addRecentFile(QStringLiteral("/a.pdf"));
        settings.setLastDirectory(QStringLiteral("/x"));
        settings.setViewState(QStringLiteral("k"), {});
        settings.setWindowGeometry(QByteArray("g"));
        QVERIFY(settings.recentFiles().isEmpty());
        QVERIFY(settings.lastDirectory().isEmpty());
        QVERIFY(!settings.viewState(QStringLiteral("k")).has_value());
        QVERIFY(settings.windowGeometry().isEmpty());
        QCOMPARE(settings.viewStateCount(), 0);
        settings.sync();
    }

    void recentFilesAreNewestFirstWithoutRepeatsAndCapped() {
        QTemporaryDir dir;
        QSettings backing(dir.filePath(QStringLiteral("s.ini")), QSettings::IniFormat);
        AppSettings settings(&backing);
        QVERIFY(settings.recentFiles().isEmpty());

        const QString a = dir.filePath(QStringLiteral("a.pdf"));
        const QString b = dir.filePath(QStringLiteral("b.pdf"));
        settings.addRecentFile(a);
        settings.addRecentFile(b);
        QCOMPARE(settings.recentFiles(), (QStringList{b, a}));
        // Opening a again moves it to the front instead of listing it twice.
        settings.addRecentFile(a);
        QCOMPARE(settings.recentFiles(), (QStringList{a, b}));
        // The same file written another way is the same entry.
        settings.addRecentFile(dir.filePath(QStringLiteral("sub/../b.pdf")));
        QCOMPARE(settings.recentFiles(), (QStringList{b, a}));

        settings.removeRecentFile(a);
        QCOMPARE(settings.recentFiles(), (QStringList{b}));

        for (int i = 0; i < 30; ++i) {
            settings.addRecentFile(dir.filePath(QStringLiteral("f%1.pdf").arg(i)));
        }
        QCOMPARE(settings.recentFiles().size(), AppSettings::kMaxRecentFiles);
        QCOMPARE(settings.recentFiles().first(), dir.filePath(QStringLiteral("f29.pdf")));
        QCOMPARE(settings.recentFiles().last(), dir.filePath(QStringLiteral("f15.pdf")));

        settings.clearRecentFiles();
        QVERIFY(settings.recentFiles().isEmpty());
    }

    void everythingSurvivesANewSettingsObject() {
        QTemporaryDir dir;
        const QString ini = dir.filePath(QStringLiteral("s.ini"));
        {
            QSettings backing(ini, QSettings::IniFormat);
            AppSettings settings(&backing);
            settings.addRecentFile(dir.filePath(QStringLiteral("a.pdf")));
            settings.setLastDirectory(dir.path());
            settings.setWindowGeometry(QByteArray("geometry"));
            settings.setViewState(QStringLiteral("key"), {7, 123.5, 1.75});
            settings.sync();
        }
        QSettings backing(ini, QSettings::IniFormat);
        const AppSettings settings(&backing);
        QCOMPARE(settings.recentFiles(), (QStringList{dir.filePath(QStringLiteral("a.pdf"))}));
        QCOMPARE(settings.lastDirectory(), dir.path());
        QCOMPARE(settings.windowGeometry(), QByteArray("geometry"));
        const auto state = settings.viewState(QStringLiteral("key"));
        QVERIFY(state.has_value());
        QCOMPARE(state->page, 7U);
        QCOMPARE(state->offsetPoints, 123.5);
        QCOMPARE(state->zoom, 1.75);
        QVERIFY(!settings.viewState(QStringLiteral("other")).has_value());
    }

    void aViewStateBelongsToOneVersionOfOneFile() {
        QTemporaryDir dir;
        const QString path = touch(dir, QStringLiteral("a.pdf"), "one");
        const QString key = AppSettings::viewKey(QFileInfo(path));
        QCOMPARE(AppSettings::viewKey(QFileInfo(path)), key);
        // The key says nothing about the path.
        QVERIFY(!key.contains(QStringLiteral("a.pdf")));
        QVERIFY(!key.contains(dir.path()));

        // Another size is another file...
        QFile file(path);
        QVERIFY(file.open(QIODevice::Append));
        file.write("more");
        file.close();
        const QString grown = AppSettings::viewKey(QFileInfo(path));
        QVERIFY(grown != key);
        // ...and so is the same size with another modification time.
        QVERIFY(file.open(QIODevice::ReadWrite));
        QVERIFY(file.setFileTime(QDateTime::currentDateTime().addDays(-3),
                                 QFileDevice::FileModificationTime));
        file.close();
        QVERIFY(AppSettings::viewKey(QFileInfo(path)) != grown);
        // Another file of the same content is another key (the path is part of it).
        QVERIFY(AppSettings::viewKey(QFileInfo(touch(dir, QStringLiteral("b.pdf"), "one"))) != key);
    }

    void viewStatesAreLeastRecentlyUsedAndCapped() {
        QTemporaryDir dir;
        QSettings backing(dir.filePath(QStringLiteral("s.ini")), QSettings::IniFormat);
        AppSettings settings(&backing);
        const int total = AppSettings::kMaxViewStates + 20;
        for (int i = 0; i < total; ++i) {
            settings.setViewState(QStringLiteral("key%1").arg(i),
                                  {static_cast<quint32>(i), 0.0, 1.0});
            if (i == 5) {
                // Used again later, so it outlives the ones written after it.
                settings.setViewState(QStringLiteral("key0"), {99, 0.0, 2.0});
            }
        }
        QCOMPARE(settings.viewStateCount(), AppSettings::kMaxViewStates);
        QVERIFY(!settings.viewState(QStringLiteral("key1")).has_value());
        QVERIFY(settings.viewState(QStringLiteral("key%1").arg(total - 1)).has_value());
        // key0 was refreshed at step 5, so it went out after key1..key5 but before the newer ones.
        QVERIFY(!settings.viewState(QStringLiteral("key0")).has_value());
        QVERIFY(settings.viewState(QStringLiteral("key%1").arg(total - 400)).has_value());
        // Writing a key again replaces it instead of adding another.
        settings.setViewState(QStringLiteral("key%1").arg(total - 1), {5, 6.0, 3.0});
        QCOMPARE(settings.viewStateCount(), AppSettings::kMaxViewStates);
        QCOMPARE(settings.viewState(QStringLiteral("key%1").arg(total - 1))->zoom, 3.0);
    }

    void damagedEntriesAreIgnoredNotTrusted() {
        QTemporaryDir dir;
        QSettings backing(dir.filePath(QStringLiteral("s.ini")), QSettings::IniFormat);
        backing.beginWriteArray(QStringLiteral("views"));
        backing.setArrayIndex(0);
        backing.setValue(QStringLiteral("key"), QStringLiteral("bad"));
        backing.setValue(QStringLiteral("page"), QStringLiteral("not a number"));
        backing.setValue(QStringLiteral("offset"), 1.0);
        backing.setValue(QStringLiteral("zoom"), 1.0);
        backing.setArrayIndex(1);
        backing.setValue(QStringLiteral("key"), QStringLiteral("nan"));
        backing.setValue(QStringLiteral("page"), 1);
        backing.setValue(QStringLiteral("offset"), QStringLiteral("nan"));
        backing.setValue(QStringLiteral("zoom"), 1.0);
        backing.setArrayIndex(2);
        backing.setValue(QStringLiteral("key"), QStringLiteral("fine"));
        backing.setValue(QStringLiteral("page"), 2);
        backing.setValue(QStringLiteral("offset"), 3.0);
        backing.setValue(QStringLiteral("zoom"), 4.0);
        backing.endArray();
        backing.setValue(QStringLiteral("files/recent"),
                         QStringList(100, QStringLiteral("/x.pdf")));

        const AppSettings settings(&backing);
        QVERIFY(!settings.viewState(QStringLiteral("bad")).has_value());
        QVERIFY(!settings.viewState(QStringLiteral("nan")).has_value());
        QCOMPARE(settings.viewState(QStringLiteral("fine"))->page, 2U);
        QCOMPARE(settings.recentFiles().size(), AppSettings::kMaxRecentFiles);
        // Nothing but finite numbers is accepted on the way in, either.
        AppSettings writable(&backing);
        writable.setViewState(QStringLiteral("inf"),
                              {0, 0.0, std::numeric_limits<double>::infinity()});
        QVERIFY(!writable.viewState(QStringLiteral("inf")).has_value());
    }

    void aViewStateKeepsHowThePagesWereArranged() {
        QTemporaryDir dir;
        QSettings backing(dir.filePath(QStringLiteral("s.ini")), QSettings::IniFormat);
        AppSettings settings(&backing);
        settings.setViewState(QStringLiteral("k"), {4, 12.5, 2.0, {false, 2, 3}});
        const auto state = settings.viewState(QStringLiteral("k"));
        QVERIFY(state.has_value());
        QCOMPARE(state->layout, (AppSettings::Layout{false, 2, 3}));
        QCOMPARE(state->page, 4U);
    }

    void stateWrittenBeforeViewModesOpensInTheUsualArrangement() {
        QTemporaryDir dir;
        QSettings backing(dir.filePath(QStringLiteral("s.ini")), QSettings::IniFormat);
        backing.beginWriteArray(QStringLiteral("views"));
        backing.setArrayIndex(0);
        backing.setValue(QStringLiteral("key"), QStringLiteral("old"));
        backing.setValue(QStringLiteral("page"), 3);
        backing.setValue(QStringLiteral("offset"), 1.0);
        backing.setValue(QStringLiteral("zoom"), 1.5);
        backing.endArray();
        const AppSettings settings(&backing);
        const auto state = settings.viewState(QStringLiteral("old"));
        QVERIFY(state.has_value());
        QCOMPARE(state->layout, (AppSettings::Layout{true, 0, 0}));
        QCOMPARE(state->zoom, 1.5);
    }

    void theLastArrangementIsKeptAndBounded() {
        QTemporaryDir dir;
        QSettings backing(dir.filePath(QStringLiteral("s.ini")), QSettings::IniFormat);
        AppSettings settings(&backing);
        QCOMPARE(settings.lastLayout(), (AppSettings::Layout{true, 0, 0}));
        settings.setLastLayout({false, 1, 2});
        QCOMPARE(settings.lastLayout(), (AppSettings::Layout{false, 1, 2}));
        // Whatever is in the file is clamped to what exists.
        backing.setValue(QStringLiteral("view/layout"), QStringList{"0", "99", "-3"});
        QCOMPARE(settings.lastLayout(), (AppSettings::Layout{false, 2, 1}));
        backing.setValue(QStringLiteral("view/layout"), QStringList{"garbage"});
        QCOMPARE(settings.lastLayout(), (AppSettings::Layout{true, 0, 0}));
        AppSettings nothing;
        nothing.setLastLayout({false, 2, 2});
        QCOMPARE(nothing.lastLayout(), (AppSettings::Layout{true, 0, 0}));
    }
    void theSidebarIsRememberedAndBounded() {
        QTemporaryDir dir;
        QSettings backing(dir.filePath(QStringLiteral("s.ini")), QSettings::IniFormat);
        AppSettings settings(&backing);
        // Hidden, with no chosen width, until the user says otherwise.
        QCOMPARE(settings.sidebar(), (AppSettings::Sidebar{false, 0, 120}));
        settings.setSidebar({true, 220, 90});
        QCOMPARE(settings.sidebar(), (AppSettings::Sidebar{true, 220, 90}));
        // Whatever is written, or found in the file, is clamped to what the sidebar can show.
        settings.setSidebar({true, 99'999, 5'000});
        QCOMPARE(settings.sidebar(), (AppSettings::Sidebar{true, AppSettings::kMaxSidebarWidth,
                                                           AppSettings::kMaxThumbnailWidth}));
        backing.setValue(QStringLiteral("sidebar/width"), 3);
        backing.setValue(QStringLiteral("sidebar/thumbnailWidth"), -7);
        QCOMPARE(settings.sidebar(), (AppSettings::Sidebar{true, AppSettings::kMinSidebarWidth,
                                                           AppSettings::kMinThumbnailWidth}));
        backing.setValue(QStringLiteral("sidebar/width"), QStringLiteral("garbage"));
        backing.setValue(QStringLiteral("sidebar/visible"), QStringLiteral("garbage"));
        QCOMPARE(settings.sidebar().width, 0);
        QVERIFY(!settings.sidebar().visible);
        AppSettings nothing;
        nothing.setSidebar({true, 200, 100});
        QCOMPARE(nothing.sidebar(), (AppSettings::Sidebar{false, 0, 120}));
    }

    void pathsAreResolvedAgainstTheDirectoryTheCommandRanIn() {
        QTemporaryDir dir;
        const QStringList resolved = vellora::resolvePaths(
            {QStringLiteral("a.pdf"), QStringLiteral("sub/../b.pdf"), QString(),
             QDir::cleanPath(dir.filePath(QStringLiteral("abs.pdf")))},
            dir.path());
        QCOMPARE(resolved, (QStringList{dir.filePath(QStringLiteral("a.pdf")),
                                        dir.filePath(QStringLiteral("b.pdf")),
                                        dir.filePath(QStringLiteral("abs.pdf"))}));
    }

    void pathComparisonFollowsThePlatform() {
        QVERIFY(vellora::samePath(QStringLiteral("/tmp/x/../a.pdf"), QStringLiteral("/tmp/a.pdf")));
        QVERIFY(!vellora::samePath(QStringLiteral("/tmp/a.pdf"), QStringLiteral("/tmp/b.pdf")));
#if defined(Q_OS_WIN) || defined(Q_OS_MACOS)
        QVERIFY(vellora::samePath(QStringLiteral("/tmp/A.pdf"), QStringLiteral("/tmp/a.pdf")));
#else
        QVERIFY(!vellora::samePath(QStringLiteral("/tmp/A.pdf"), QStringLiteral("/tmp/a.pdf")));
#endif
    }
};

QTEST_MAIN(TstSettings)
#include "tst_settings.moc"