// R27 Error Capture content script (runs at DocumentIdle / LoadFinished).
// Captures page JS errors, unhandled rejections, console.error/warn, and
// every fetch/XHR URL, into a fixed on-page banner + document.title.
// Round 27 adds: scheduler/postTask, serviceWorker, visibilityState,
// and a live IntersectionObserver delivery probe on the real page.
// Read-only: it does not modify page behaviour except adding the banner
// and one 1px sentinel div for the IO probe.
(function () {
  var box = document.createElement('div');
  box.id = 'r27-errcap';
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
    return 'R27-ERRCAP active. IO=' + (typeof IntersectionObserver) +
      ' RIC=' + (typeof requestIdleCallback) +
      ' SCH=' + sch + ' postTask=' + pt + ' SW=' + sw + ' VIS=' + vis;
  }
  box.textContent = firstLine();
  (document.body || document.documentElement).appendChild(box);

  var lines = [];
  function log(kind, msg) {
    lines.push(kind + ': ' + msg);
    if (lines.length > 40) lines.shift();
    box.textContent = firstLine() + '\n' + lines.join('\n');
    try { document.title = 'R27: ' + lines[lines.length - 1].slice(0, 120); } catch (e) {}
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

  // Log every fetch/XHR so we can see whether the feed request is attempted.
  var ofetch = window.fetch;
  if (ofetch) {
    window.fetch = function (input, init) {
      var url = (typeof input === 'string') ? input : (input && input.url) || String(input);
      var method = (init && init.method) || (input && input.method) || 'GET';
      log('FETCH', method + ' ' + String(url).slice(0, 160));
      return ofetch.apply(this, arguments);
    };
  }
  var oopen = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url) {
    log('XHR', method + ' ' + String(url).slice(0, 160));
    return oopen.apply(this, arguments);
  };

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
  try { log('READY', 'readyState=' + document.readyState); } catch (e) {}
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
      sentinel.id = 'r27-io-sentinel';
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
