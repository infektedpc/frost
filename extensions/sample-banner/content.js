// Frost Sample Banner — Round 3 sample content script.
// Adds a visible banner at the top of example.org pages, so the
// extension-host gate can see the script run.
(function () {
  if (document.getElementById("frost-sample-banner")) return;
  var banner = document.createElement("div");
  banner.id = "frost-sample-banner";
  banner.textContent = "❄ Frost sample extension is live";
  banner.style.cssText =
    "position:fixed;top:0;left:0;right:0;z-index:9999;" +
    "background:#7ec8ff;color:#00233d;font:bold 16px sans-serif;" +
    "text-align:center;padding:10px 12px;border-bottom:3px solid #0057d9;";
  document.body.insertBefore(banner, document.body.firstChild);
})();
