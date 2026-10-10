// A PDF of text pages for tests: `contents` are the page content streams, each page 612 x 792
// points with Helvetica as /F1, turned `rotate` degrees. Small enough to read, real enough for
// PDFium.
#pragma once

#include <QByteArray>
#include <QList>
#include <QString>
#include <QStringList>

inline QByteArray textPdf(const QStringList& contents, int rotate = 0) {
    QStringList objects;
    objects << QStringLiteral("<< /Type /Catalog /Pages 2 0 R >>");
    QStringList kids;
    for (int i = 0; i < contents.size(); ++i) {
        kids << QStringLiteral("%1 0 R").arg(3 + i);
    }
    objects << QStringLiteral("<< /Type /Pages /Kids [%1] /Count %2 >>")
                   .arg(kids.join(QLatin1Char(' ')))
                   .arg(contents.size());
    const int fontObject = 3 + 2 * static_cast<int>(contents.size());
    for (int i = 0; i < contents.size(); ++i) {
        objects << QStringLiteral("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Rotate %1 "
                                  "/Contents %2 0 R /Resources << /Font << /F1 %3 0 R >> >> >>")
                       .arg(rotate)
                       .arg(3 + contents.size() + i)
                       .arg(fontObject);
    }
    for (const QString& content : contents) {
        objects << QStringLiteral("<< /Length %1 >>\nstream\n%2\nendstream")
                       .arg(content.size())
                       .arg(content);
    }
    objects << QStringLiteral(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>");
    QByteArray out = "%PDF-1.4\n";
    QList<qsizetype> offsets;
    for (int i = 0; i < objects.size(); ++i) {
        offsets << out.size();
        out += QStringLiteral("%1 0 obj\n%2\nendobj\n").arg(i + 1).arg(objects.at(i)).toUtf8();
    }
    const qsizetype xref = out.size();
    out += QStringLiteral("xref\n0 %1\n0000000000 65535 f \n").arg(objects.size() + 1).toUtf8();
    for (const qsizetype offset : offsets) {
        out += QStringLiteral("%1 00000 n \n").arg(offset, 10, 10, QLatin1Char('0')).toUtf8();
    }
    out += QStringLiteral("trailer\n<< /Size %1 /Root 1 0 R >>\nstartxref\n%2\n%%EOF\n")
               .arg(objects.size() + 1)
               .arg(xref)
               .toUtf8();
    return out;
}
