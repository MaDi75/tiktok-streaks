// Минимальный сервер-приёмник анонимной статистики TikTok Streaks.
// Без внешних зависимостей — только встроенный модуль http.
//
// Запуск:   node server.js
// Порт:     переменная окружения PORT (по умолчанию 8787)
// Лог:      файл pings.jsonl рядом со скриптом (по строке JSON на каждый пинг)
//
// Клиент (приложение) шлёт POST на /ping с телом:
//   { "installId": "...", "version": "0.2.0", "os": "windows" }
// Сервер дописывает к записи IP соединения и время.
//
// IP определяется так: за обратным прокси (nginx/Cloudflare) берётся первый
// адрес из X-Forwarded-For, иначе — адрес сокета.

const http = require('http');
const fs = require('fs');
const path = require('path');

const PORT = process.env.PORT || 8787;
const LOG_FILE = path.join(__dirname, 'pings.jsonl');

function clientIp(req) {
  const fwd = req.headers['x-forwarded-for'];
  if (fwd) return String(fwd).split(',')[0].trim();
  return (req.socket.remoteAddress || '').replace(/^::ffff:/, '');
}

const server = http.createServer((req, res) => {
  if (req.method === 'POST' && req.url === '/ping') {
    let body = '';
    req.on('data', chunk => {
      body += chunk;
      if (body.length > 10_000) req.destroy(); // защита от слишком больших тел
    });
    req.on('end', () => {
      let payload = {};
      try { payload = JSON.parse(body || '{}'); } catch {}
      const record = {
        time: new Date().toISOString(),
        ip: clientIp(req),
        installId: String(payload.installId || '').slice(0, 64),
        version: String(payload.version || '').slice(0, 32),
        os: String(payload.os || '').slice(0, 32),
      };
      fs.appendFile(LOG_FILE, JSON.stringify(record) + '\n', () => {});
      console.log(`${record.time}  ${record.ip}  v${record.version}  ${record.os}  ${record.installId}`);
      res.writeHead(204);
      res.end();
    });
    return;
  }

  // Простая сводка: GET /stats — сколько пингов и сколько уникальных установок/IP.
  if (req.method === 'GET' && req.url === '/stats') {
    let lines = [];
    try { lines = fs.readFileSync(LOG_FILE, 'utf8').trim().split('\n').filter(Boolean); } catch {}
    const installs = new Set(), ips = new Set();
    for (const l of lines) {
      try { const r = JSON.parse(l); installs.add(r.installId); ips.add(r.ip); } catch {}
    }
    res.writeHead(200, { 'content-type': 'application/json; charset=utf-8' });
    res.end(JSON.stringify({ pings: lines.length, installs: installs.size, ips: ips.size }, null, 2));
    return;
  }

  res.writeHead(404);
  res.end();
});

server.listen(PORT, () => console.log(`TikTok Streaks telemetry server на порту ${PORT}`));
