// A window with a title and something drawn in it; it says so on stdout.
// ELECTRON_HELLO_QUIT=<seconds> closes it after that long.
const { app, BrowserWindow } = require("electron");

app.whenReady().then(() => {
  const win = new BrowserWindow({ width: 640, height: 400, title: "electron-hello", backgroundColor: "#2a6fc8", show: false });
  const page = "<!doctype html><title>electron-hello</title><body style='margin:0;background:#2a6fc8;color:#fff;font:32px sans-serif;display:grid;place-items:center;height:100vh'>Hello from a bundle</body>";
  win.loadURL("data:text/html;charset=utf-8," + encodeURIComponent(page));
  win.once("ready-to-show", () => {
    win.show();
    console.log(`electron-hello 0.5.0: a window (Electron ${process.versions.electron.split(".")[0]}, packaged ${app.isPackaged})`);
    const quit = Number(process.env.ELECTRON_HELLO_QUIT || 0);
    if (quit > 0) setTimeout(() => app.quit(), quit * 1000);
  });
});

app.on("window-all-closed", () => app.quit());
