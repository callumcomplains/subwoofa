// Starts the page once every script above has loaded.

init().catch((e) => log(String(e), "bad"));
initTools();
initVerify();
initAtmos();
