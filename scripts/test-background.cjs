const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../chrome-extension/background.js'), 'utf8');
const extensionUrl = page => `chrome-extension://fixture/${page}`;
const extensionControlsProxy = 'controlled_by_this_extension';
const proxyIsAvailable = 'controllable_by_this_extension';

function fixture() {
  const state = { level: proxyIsAvailable, running: false };
  let receiveRequest;
  let receiveNativeResponse;

  const port = {
    onMessage: { addListener: listener => { receiveNativeResponse = listener; } },
    onDisconnect: { addListener() {} },
    disconnect() {},
    postMessage({ action }) {
      queueMicrotask(() => {
        if (action === 'start') state.running = true;
        const response = action === 'save' ? state.save() : { running: state.running };
        receiveNativeResponse(response);
      });
    },
  };
  const chrome = {
    runtime: {
      getURL: extensionUrl,
      connectNative: () => port,
      onMessage: { addListener: listener => { receiveRequest = listener; } },
      onStartup: { addListener() {} },
      onInstalled: { addListener() {} },
    },
    proxy: {
      settings: {
        async get() { return { levelOfControl: state.level }; },
        async set() {
          assert.equal(state.running, true);
          state.level = extensionControlsProxy;
        },
        async clear() { state.level = proxyIsAvailable; },
      },
      onProxyError: { addListener() {} },
    },
  };

  vm.runInNewContext(source, { chrome, console, TextEncoder });
  const send = (page, action, payload) => new Promise(resolve => {
    receiveRequest({ action, payload }, { url: extensionUrl(page) }, resolve);
  });
  return { state, send };
}

test('enables Chrome routing after the helper reports successful startup', async () => {
  const { state, send } = fixture();
  const result = await send('popup.html', 'start');
  assert.equal(result.running, true);
  assert.equal(state.level, extensionControlsProxy);
});

test('keeps Chrome routing when a failed save restores the previous proxy', async () => {
  const { state, send } = fixture();
  await send('popup.html', 'start');
  state.save = () => ({ error: 'Не удалось сохранить список.' });

  const result = await send('options.html', 'save', {});
  assert.equal(result.error, 'Не удалось сохранить список.');
  assert.equal(state.level, extensionControlsProxy);
});

test('clears Chrome routing when saving leaves the proxy unavailable', async () => {
  const { state, send } = fixture();
  await send('popup.html', 'start');
  state.save = () => {
    state.running = false;
    return { error: 'Восстановить предыдущий запуск не удалось.' };
  };

  const result = await send('options.html', 'save', {});
  assert.match(result.error, /Восстановить предыдущий запуск не удалось/);
  assert.match(result.error, /Настройки прокси Chrome сброшены/);
  assert.equal(state.level, proxyIsAvailable);
});
