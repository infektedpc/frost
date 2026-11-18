// R25 Error Capture content script (runs at DocumentIdle / LoadFinished).
// Captures page JS errors, unhandled rejections, console.error/warn, and
// every fetch/XHR URL, into a fixed on-page banner + document.title.
// Read-only: it does not modify page behaviour except adding the banner.
(function () {
  var box = document.createElement('div');
  box.id = 'r25-errcap';
  box.style.cssText = 'position:fixed;top:0;left:0;right:0;z-index:2147483647;' +
    'background:#7a0000;color:#fff;font:14px/1.4 monospace;padding:6px 10px;' +
    'max-height:220px;overflow:auto;white-space:pre-wrap;word-break:break-all;';
  box.textContent = 'R25-ERRCAP active. IO=' + (typeof IntersectionObserver) +
    ' RIC=' + (typeof requestIdleCallback);
  (document.body || document.documentElement).appendChild(box);

  var lines = [];
  function log(kind, msg) {
    lines.push(kind + ': ' + msg);
    if (lines.length > 40) lines.shift();
    box.textContent = 'R25-ERRCAP active. IO=' + (typeof IntersectionObserver) +
      ' RIC=' + (typeof requestIdleCallback) + '\n' + lines.join('\n');
    try { document.title = 'R25: ' + lines[lines.length - 1].slice(0, 120); } catch (e) {}
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
})();
