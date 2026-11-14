const form = document.getElementById('settings');
const ui = Object.fromEntries(['interface', 'sites', 'message', 'save', 'import', 'json-file']
  .map(id => [id, document.getElementById(id)]));
let loaded = false;

function setBusy(busy) {
  ui.save.disabled = busy || !loaded;
  ui.import.disabled = busy || !loaded;
  ui.interface.disabled = busy || !loaded;
  ui.sites.disabled = busy || !loaded;
}

async function api(action, payload) {
  const result = await chrome.runtime.sendMessage({ action, payload });
  if (result.error) throw new Error(result.error);
  return result;
}

async function act(operation) {
  setBusy(true);
  ui.message.dataset.error = false;
  ui.message.textContent = '';
  try {
    await operation();
  } catch (error) {
    ui.message.dataset.error = true;
    ui.message.textContent = error.message.includes('Specified native messaging host not found')
      ? 'Установи помощник Chrome по инструкции в README и обнови страницу.'
      : error.message;
  } finally {
    setBusy(false);
  }
}

function showSettings({ settings }) {
  ui.interface.value = settings.interface;
  ui.sites.value = settings.sites.join('\n');
}

form.addEventListener('submit', event => {
  event.preventDefault();
  const payload = { interface: ui.interface.value, sites: ui.sites.value };
  act(async () => {
    showSettings(await api('save', payload));
    ui.message.textContent = 'Список сохранён.';
  });
});

ui.import.addEventListener('click', () => ui['json-file'].click());
ui['json-file'].addEventListener('change', event => {
  const [file] = event.target.files;
  event.target.value = '';
  if (!file) return;

  act(async () => {
    ui.message.textContent = 'Читаю JSON…';
    const entries = JSON.parse((await file.text()).replace(/^\uFEFF/, ''));
    const { sites } = await api('import', entries);
    ui.sites.value = sites.join('\n');
    ui.message.textContent = `Загружено правил — ${sites.length}. Нажми «Сохранить список», чтобы применить их.`;
  });
});

act(async () => {
  showSettings(await api('settings'));
  loaded = true;
});
