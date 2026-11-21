// justOS: a file downloaded in Firefox opens as a double click in the Files
// app does, through the system's default handler (justos-open): no "Choose
// Application" window. Programs and installers still get Firefox's "Open
// Executable File?" question once, before they run. Downloads go to the
// folder the Files app shows as "Scaricati".
pref("browser.download.always_ask_before_handling_new_types", false);
pref("browser.download.useDownloadDir", true);
pref("browser.download.folderList", 2);
pref("browser.download.dir", "/home/Scaricati");

// Websites see a Windows PC: download pages (programs, drivers, games)
// then offer their Windows version, which runs here with Wine, instead of
// a Linux one that would rarely install. The version in it must follow
// Firefox's own (rootfs/../LINUX_PKGS: firefox-esr 140), so that sites
// that check it keep working; scripts see the same through
// navigator.platform and navigator.oscpu.
pref("general.useragent.override", "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0");
pref("general.platform.override", "Win32");
pref("general.oscpu.override", "Windows NT 10.0; Win64; x64");
pref("general.appversion.override", "5.0 (Windows)");
