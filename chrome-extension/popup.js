const toggle = document.getElementById('toggle');
const status = document.getElementById('status');
const error = document.getElementById('error');
const panel = document.getElementById('panel');
let nextAction = 'start';

function render({ running, level }) {
  const connected = level === 'controlled_by_this_extension';
  const available = level === 'controllable_by_this_extension' || connected;
  nextAction = running || connected ? 'stop' : 'start';
  toggle.disabled = !available && nextAction === 'start';
  toggle.textContent = nextAction === 'stop' ? 'Выключить' : 'Включить';
  if (running && connected) {
    status.textContent = 'sing-box включён для Chrome';
  } else if (level === 'controlled_by_other_extensions') {
    status.textContent = 'Прокси управляет другое расширение';
  } else if (!available) {
    status.textContent = 'Chrome ограничил изменение настроек прокси';
  } else {
    status.textContent = running ? 'sing-box запущен, Chrome не подключён' : 'sing-box выключен';
  }
}

async function send(action) {
  const result = await chrome.runtime.sendMessage({ action });
  if (result.error) throw new Error(result.error);
  return result;
}

async function update(action) {
  toggle.disabled = true;
  error.hidden = true;
  if (action === 'start') status.textContent = 'Запускаю sing-box…';
  if (action === 'stop') status.textContent = 'Останавливаю sing-box…';
  try {
    render(await send(action));
  } catch (cause) {
    const message = cause.message || String(cause);
    if (message.includes('Specified native messaging host not found')) {
      error.textContent = 'Установи помощник Chrome по инструкции в README.';
    } else {
      error.textContent = message;
    }
    error.hidden = false;
    status.textContent = 'Не удалось получить состояние sing-box';
    if (action !== 'status') {
      try {
        render(await send('status'));
      } catch {
        toggle.disabled = false;
      }
    } else {
      toggle.disabled = false;
    }
  }
}

toggle.addEventListener('click', () => update(nextAction));
panel.addEventListener('click', () => chrome.runtime.openOptionsPage());
update('status');
