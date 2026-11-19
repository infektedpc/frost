// R29 Error Capture content script (runs at DocumentStart / HeadParsed).
// Captures page JS errors, unhandled rejections, console.error/warn, and
// every fetch/XHR/sendBeacon call, into a fixed on-page banner +
// document.title. Every line is stamped with ms since install, so the
// bootstrap sequence is visible: what fires first, what the app does in
// its first seconds, and where it goes quiet before `browse`.
// Round 28 moves injection from DocumentIdle to DocumentStart: at load
// finish the app had already fired guide/log_event and cached its own
// fetch reference, so the R27 banner saw nothing. Read-only: it does not
// modify page behaviour except adding the banner and one 1px sentinel
// div for the IO probe.
// Round 29: the banner keeps 200 lines (was 40 — the cap ate the early
// bootstrap), and every line is ALSO mirrored to console.log with an
// ERRCAP prefix, so the full bootstrap chain lands in the engine log
// via the new console bridge (frozen FrostDelegate::show_console_message)
// and can be read even when the banner itself cannot (the Inspector
// snapshot wedges on YouTube's DOM).
(function () {
  var t0 = performance.now();
  function stamp() { return '+' + (performance.now() - t0).toFixed(0) + 'ms '; }

  var box = document.createElement('div');
  box.id = 'r29-errcap';
  box.style.cssText = 'position:fixed;top:0;left:0;right:0;z-index:2147483647;' +
    'background:#7a0000;color:#fff;font:14px/1.4 monospace;padding:6px 10px;' +
    'max-height:220px;overflow:auto;white-space:pre-wrap;word-break:break-all;';

  // Round 27: the browse-trigger candidates, as one status line.
  function firstLine() {
    var sch = (typeof scheduler === 'undefined') ? 'undefined' : typeof scheduler;
    var pt = 'n/a';
    try { pt = (typeof scheduler !== 'undefined' && scheduler) ? typeof scheduler.postTask : 'n/a'; } catch (e) { pt = 'err'; }
    var sw = 'n/a';
    try { sw = (navigator && navigator.serviceWorker) ? typeof navigator.serviceWorker : 'undefined'; } catch (e) { sw = 'err'; }
    var vis = 'n/a';
    try { vis = document.visibilityState; } catch (e) { vis = 'err'; }
    return 'R29-ERRCAP active. IO=' + (typeof IntersectionObserver) +
      ' RIC=' + (typeof requestIdleCallback) +
      ' SCH=' + sch + ' postTask=' + pt + ' SW=' + sw + ' VIS=' + vis;
  }
  box.textContent = firstLine();
  (document.body || document.documentElement).appendChild(box);

  var lines = [];
  function log(kind, msg) {
    lines.push(stamp() + kind + ': ' + msg);
    if (lines.length > 200) lines.shift();
    box.textContent = firstLine() + '\n' + lines.join('\n');
    try { document.title = 'R29: ' + lines[lines.length - 1].slice(0, 120); } catch (e) {}
    // Round 29: mirror every line to the engine log via console.log
    // (frozen logs ERRCAP-prefixed console messages at info level).
    // console.log itself is not wrapped by this script, so no recursion.
    try { console.log('ERRCAP ' + stamp() + kind + ': ' + msg); } catch (e) {}
  }

  window.addEventListener('error', function (ev) {
    log('ERR', (ev.message || 'unknown') + ' @ ' + (ev.filename || '') + ':' + (ev.lineno || ''));
  });
  window.addEventListener('unhandledrejection', function (ev) {
    var r = ev.reason;
    log('REJ', (r && r.message) ? r.message : String(r));
  });
  ['error', 'warn'].forEach(function (lvl) {
    var orig = console[lvl];
    console[lvl] = function () {
      try { log('CONSOLE.' + lvl, Array.prototype.map.call(arguments, function (a) {
        return (a && a.message) ? a.message : String(a);
      }).join(' ').slice(0, 300)); } catch (e) {}
      return orig.apply(console, arguments);
    };
  });

  // Round 28: log readyState transitions — the bootstrap timeline anchors.
  try {
    log('READY', 'readyState=' + document.readyState);
    document.addEventListener('readystatechange', function () {
      log('RS', 'readyState=' + document.readyState);
    });
  } catch (e) {}

  // Log every fetch/XHR/sendBeacon so we can see whether the feed request
  // is attempted, and WHEN relative to install.
  var ofetch = window.fetch;
  if (ofetch) {
    window.fetch = function (input, init) {
      var url = (typeof input === 'string') ? input : (input && input.url) || String(input);
      var method = (init && init.method) || (input && input.method) || 'GET';
      // Round 28: log whether the caller's abort signal is already
      // aborted — a pre-aborted fetch rejects without ever reaching
      // the network, which would explain a banner attempt with no
      // engine-log POST (the R26 player-POST mystery).
      var sig = '';
      try {
        var s = (init && init.signal) || (input && input.signal);
        if (s) sig = ' signal.aborted=' + s.aborted;
      } catch (e) {}
      log('FETCH', method + ' ' + String(url).slice(0, 160) + sig);
      var p;
      try {
        p = ofetch.apply(this, arguments);
      } catch (e) {
        log('FETCH-THROW', String((e && e.message) || e).slice(0, 120));
        throw e;
      }
      // Round 28: log the outcome — a fetch that never settles, or
      // rejects at once, never reaches http_loader and leaves no
      // engine-log trace at all.
      try {
        var shortUrl = String(url).slice(0, 80);
        p.then(function (r) {
          log('FETCH-OK', r.status + ' ' + shortUrl);
        }, function (e) {
          log('FETCH-ERR', (e && e.name ? e.name + ': ' : '') + String((e && e.message) || e).slice(0, 120) + ' ' + shortUrl);
        });
      } catch (e) {}
      return p;
    };
  } else {
    log('FETCH', 'window.fetch undefined at install');
  }
  var oopen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url) {
    log('XHR', method + ' ' + String(url).slice(0, 160));
    // Round 28: log the outcome — distinguishes "never sent" from
    // "sent and failed".
    try {
      var shortUrl = String(url).slice(0, 80);
      var self = this;
      var done = function (ev) {
        log('XHR-' + (ev.type === 'load' ? 'OK ' + self.status : ev.type.toUpperCase()), shortUrl);
      };
      this.addEventListener('load', done);
      this.addEventListener('error', done);
      this.addEventListener('abort', done);
      this.addEventListener('timeout', done);
    } catch (e) {}
    return oopen.apply(this, arguments);
  };

  // Round 28: sendBeacon is another transport the app may use for
  // telemetry (log_event) — wrap it too so the bootstrap is fully seen.
  try {
    if (navigator && navigator.sendBeacon) {
      var obeacon = navigator.sendBeacon;
      navigator.sendBeacon = function (url, data) {
        log('BEACON', String(url).slice(0, 160));
        return obeacon.apply(this, arguments);
      };
    } else {
      log('BEACON', 'navigator.sendBeacon undefined at install');
    }
  } catch (e) {}

  // Round 27: log serviceWorker registration attempts, if the API exists.
  try {
    if (navigator && navigator.serviceWorker && navigator.serviceWorker.register) {
      var oreg = navigator.serviceWorker.register;
      navigator.serviceWorker.register = function () {
        log('SWREG', String(arguments[0]).slice(0, 160));
        return oreg.apply(this, arguments);
      };
    }
  } catch (e) {}

  // Round 27: requestIdleCallback delivery probe on the real page
  // (scheduler.js uses rIC; if rIC callbacks never run here, the
  // app's own scheduler stalls). Also record readyState at install.
  try {
    if (typeof requestIdleCallback === 'function') {
      var ricFired = false;
      requestIdleCallback(function (d) {
        ricFired = true;
        log('RIC-CB', 'fired timeRemaining=' + d.timeRemaining() + ' didTimeout=' + d.didTimeout);
      });
      setTimeout(function () {
        if (!ricFired) log('RIC-CB', 'NOT FIRED after 10s (idle delivery broken on this page)');
      }, 10000);
    } else {
      log('RIC-CB', 'requestIdleCallback not a function, cannot probe');
    }
  } catch (e) {
    log('RIC-CB', 'probe error: ' + String((e && e.message) || e));
  }

  // Round 27: live IntersectionObserver delivery probe on the real page.
  // Native IO is enabled since R26, so this tests whether IO callbacks
  // actually fire here (the R25 polyfill question, re-asked for native).
  try {
    if (typeof IntersectionObserver === 'function') {
      var sentinel = document.createElement('div');
      sentinel.id = 'r29-io-sentinel';
      sentinel.style.cssText = 'position:fixed;top:100px;left:10px;width:50px;height:50px;background:rgba(0,255,0,0.01);z-index:2147483646;';
      (document.body || document.documentElement).appendChild(sentinel);
      var ioFired = false;
      var io = new IntersectionObserver(function (entries) {
        ioFired = true;
        log('IO-CB', 'fired entries=' + entries.length +
          ' isIntersecting=' + (entries[0] && entries[0].isIntersecting) +
          ' ratio=' + (entries[0] && entries[0].intersectionRatio));
      });
      io.observe(sentinel);
      setTimeout(function () {
        if (!ioFired) log('IO-CB', 'NOT FIRED after 5s (observer delivery broken on this page)');
      }, 5000);
    } else {
      log('IO-CB', 'IntersectionObserver not a function, cannot probe');
    }
  } catch (e) {
    log('IO-CB', 'probe error: ' + String((e && e.message) || e));
  }
})();
