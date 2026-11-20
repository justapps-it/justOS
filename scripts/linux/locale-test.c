/* The language Linux programs get on justOS (scripts/test-linux.sh). The
 * kernel starts them with LANG=it_IT.UTF-8 when the system has glibc's
 * locale of that name (tools/linux/debroot.py --locales compiles it into
 * the tree), so setlocale has to find it, in UTF-8 with Italian numbers
 * and dates; and the messages of glibc and of GTK have to come in Italian,
 * from the translations the tree keeps. Linked dynamically against glibc. */
#include <errno.h>
#include <langinfo.h>
#include <libintl.h>
#include <locale.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

int main(void) {
    const char *set = setlocale(LC_ALL, "");
    const char *lang = getenv("LANG");
    printf("lc: LANG=%s set=%s\n", lang ? lang : "-", set ? set : "-");
    printf("lc: codeset=%s number=%'.2f\n", nl_langinfo(CODESET), 1234567.891);
    struct tm day = {.tm_year = 126, .tm_mon = 10, .tm_mday = 20, .tm_wday = 5};
    char text[64];
    strftime(text, sizeof text, "%A %e %B %Y", &day);
    printf("lc: date=%s\n", text);
    printf("lc: error=%s\n", strerror(ENOENT));
    /* GTK's own translations, as a GTK program asks for them. */
    bindtextdomain("gtk30", "/usr/share/locale");
    bind_textdomain_codeset("gtk30", "UTF-8");
    printf("lc: gtk=%s %s\n", dgettext("gtk30", "_Cancel"), dgettext("gtk30", "Recent"));
    return 0;
}
