const host = 'org.vpn_ru_proxy.chrome';
const settings = chrome.proxy.settings;
const scope = 'regular_only';
const controlledHere = 'controlled_by_this_extension';
const config = {
  mode: 'fixed_servers',
  rules: {
    singleProxy: { scheme: 'http', host: '127.0.0.1', port: 17892 },
  },
};

// Serialize requests from the popup and settings tabs.
let queue = Promise.resolve();
let port;
let pending;

function enqueue(operation) {
  const result = queue.then(operation);
  queue = result.catch(() => {});
  return result;
}

async function proxyLevel() {
  const { levelOfControl } = await settings.get({ incognito: false });
  return levelOfControl;
}

function clearProxy() {
  return settings.clear({ scope });
}

async function clearInactiveProxy() {
  if (!port) await clearProxy();
}

function connect() {
  const connection = chrome.runtime.connectNative(host);
  port = connection;
  connection.onMessage.addListener(response => {
    if (port !== connection || !pending) return;
    const { resolve, reject } = pending;
    pending = undefined;
    if (response.error) reject(new Error(response.error));
    else resolve(response);
  });
  connection.onDisconnect.addListener(() => {
    const reason = chrome.runtime.lastError?.message;
    if (port !== connection) return;
    port = undefined;
    pending?.reject(new Error(reason || 'Связь с помощником VPN RU Proxy разорвана.'));
    pending = undefined;
    enqueue(clearInactiveProxy).catch(console.error);
  });
}

function disconnect() {
  const connection = port;
  port = undefined;
  connection?.disconnect();
}

function requestNative(action, payload) {
  const message = { action, payload };
  if (payload !== undefined && new TextEncoder().encode(JSON.stringify(message)).length > 4 * 1024 * 1024) {
    throw new Error('Слишком большой список сайтов. Сократи список и попробуй снова.');
  }
  if (!port) {
    // Settings requests while off use a short-lived native process.
    return chrome.runtime.sendNativeMessage(host, message).then(response => {
      if (response.error) throw new Error(response.error);
      return response;
    });
  }
  return new Promise((resolve, reject) => {
    port.postMessage(message);
    pending = { resolve, reject };
  });
}

async function status() {
  let running = false;
  if (port) {
    ({ running } = await requestNative('status'));
    if (!running) disconnect();
  }
  let level = await proxyLevel();
  if (!running && level === controlledHere) {
    await clearProxy();
    level = await proxyLevel();
  }
  return { running, level };
}

async function start() {
  const level = await proxyLevel();
  if (level !== 'controllable_by_this_extension' && level !== controlledHere) {
    throw new Error('Chrome ограничил изменение настроек прокси.');
  }
  try {
    if (!port) connect();
    const { running } = await requestNative('start');
    if (!running) throw new Error('sing-box не запустился.');
    await settings.set({ scope, value: config });
    if ((await proxyLevel()) !== controlledHere) {
      throw new Error('Не удалось применить настройки прокси в Chrome.');
    }
    return { running: true, level: controlledHere };
  } catch (error) {
    await clearProxy().catch(console.error);
    if (port) await requestNative('stop').catch(console.error);
    disconnect();
    throw error;
  }
}

async function stop() {
  await clearProxy();
  if (port) {
    try {
      await requestNative('stop');
    } finally {
      disconnect();
    }
  }
  return { running: false, level: await proxyLevel() };
}

chrome.runtime.onMessage.addListener((request, sender, reply) => {
  if (!request || typeof request.action !== 'string') return;
  const { action, payload } = request;
  const fromPopup = sender.url === chrome.runtime.getURL('popup.html');
  const fromOptions = sender.url === chrome.runtime.getURL('options.html');
  const popupActions = { status, start, stop };
  let operation;
  if (fromPopup && Object.hasOwn(popupActions, action)) {
    operation = popupActions[action];
  } else if (fromOptions && ['settings', 'save', 'import'].includes(action)) {
    operation = () => requestNative(action, payload);
  } else {
    return;
  }
  enqueue(operation).then(reply, error => reply({ error: error.message }));
  return true;
});

chrome.runtime.onStartup.addListener(() => enqueue(clearInactiveProxy).catch(console.error));
chrome.runtime.onInstalled.addListener(() => enqueue(clearInactiveProxy).catch(console.error));
chrome.proxy.onProxyError.addListener(() => {
  if (port) enqueue(status).catch(console.error);
});
