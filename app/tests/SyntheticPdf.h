// A flat n-page PDF for tests: every page is US Letter with "Page N" and a frame, in one flat
// /Pages node. A cheap stand-in for the real generators of M0 task 24 (bench/).
#pragma once

#include <QByteArray>
#include <QVector>

inline QByteArray syntheticPdf(int pages) {
    QByteArray out("%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
    QVector<qsizetype> offsets(4 + 2 * pages, 0);
    const auto object = [&](int number, const QByteArray& body) {
        offsets[number] = out.size();
        out += QByteArray::number(number) + " 0 obj\n" + body + "\nendobj\n";
    };
    // 1 catalog, 2 pages, 3 font; then a page object and a content stream for each page.
    object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    QByteArray kids;
    for (int i = 0; i < pages; ++i) {
        kids += QByteArray::number(4 + 2 * i) + " 0 R ";
    }
    object(2, "<< /Type /Pages /Kids [" + kids + "] /Count " + QByteArray::number(pages) +
                  " /MediaBox [0 0 612 792] >>");
    object(3, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    for (int i = 0; i < pages; ++i) {
        const int page = 4 + 2 * i;
        const QByteArray text = "BT /F1 48 Tf 72 700 Td (Page " + QByteArray::number(i + 1) +
                                ") Tj ET 72 72 468 600 re S";
        object(page, "<< /Type /Page /Parent 2 0 R /Contents " + QByteArray::number(page + 1) +
                         " 0 R /Resources << /Font << /F1 3 0 R >> >> >>");
        object(page + 1, "<< /Length " + QByteArray::number(text.size()) + " >>\nstream\n" + text +
                             "\nendstream");
    }
    const int size = 4 + 2 * pages;
    const qsizetype xref = out.size();
    out += "xref\n0 " + QByteArray::number(size) + "\n0000000000 65535 f \n";
    for (int number = 1; number < size; ++number) {
        out += QByteArray::number(offsets[number]).rightJustified(10, '0') + " 00000 n \n";
    }
    out += "trailer\n<< /Size " + QByteArray::number(size) + " /Root 1 0 R >>\nstartxref\n" +
           QByteArray::number(xref) + "\n%%EOF\n";
    return out;
}
