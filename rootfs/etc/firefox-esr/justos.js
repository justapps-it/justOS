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

// Less memory (justOS runs on computers with little of it): no web
// content processes started ahead of time (Firefox keeps three waiting,
// 16 MiB each), one process per site however many of its tabs are open
// (sites stay apart from each other), at most two pages kept ready for
// Back, a smaller memory cache, and tabs unloaded when memory runs low
// (Firefox reads MemAvailable in /proc/meminfo); the add-ons run in
// Firefox's own process instead of one more (35 MiB). No sponsored or
// recommended stories on the new tab page and no telemetry: nothing
// fetched or sent for them.
pref("dom.ipc.processPrelaunch.enabled", false);
pref("extensions.webextensions.remote", false);
pref("dom.ipc.processPrelaunch.fission.number", 0);
pref("dom.ipc.processCount.webIsolated", 1);
pref("browser.sessionhistory.max_total_viewers", 2);
pref("browser.cache.memory.capacity", 16384);
pref("browser.tabs.unloadOnLowMemory", true);
pref("browser.newtabpage.activity-stream.feeds.section.topstories", false);
pref("browser.newtabpage.activity-stream.feeds.topsites", true);
pref("browser.newtabpage.activity-stream.showSponsored", false);
pref("browser.newtabpage.activity-stream.showSponsoredTopSites", false);
pref("browser.newtabpage.activity-stream.feeds.telemetry", false);
pref("browser.newtabpage.activity-stream.telemetry", false);
pref("browser.ping-centre.telemetry", false);
pref("toolkit.telemetry.unified", false);
pref("datareporting.healthreport.uploadEnabled", false);
pref("datareporting.policy.dataSubmissionEnabled", false);
pref("app.normandy.enabled", false);
pref("extensions.pocket.enabled", false);
