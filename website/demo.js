/* Hero demo: spotlight-driven step-through of the decompile pipeline.
   Each rule gets two beats: FOCUS (matched lines bright with a blue bar,
   everything else dimmed) then APPLY (replacements land in green, dim held).
   ▸/◂ walk the beats; the dots jump between stages. No autoplay. */
(function () {
  "use strict";

  var codeEl = document.getElementById("demo-code");
  var statusEl = document.getElementById("demo-status");
  var prevBtn = document.getElementById("step-prev");
  var nextBtn = document.getElementById("step-next");
  var dotsEl = document.getElementById("step-dots");
  if (!codeEl || !prevBtn) return;

  /* ---- tiny highlighter (fixed, known sample only) ---- */
  var KW = "function|return|var|const|let|import|export|from|async|await|yield|new|try|catch|void|this|arguments|null|true|false";
  var TOKEN_RE = new RegExp(
    '("[^"]*")|(\\/\\/.*$)|(\\/\\*[\\s\\S]*?\\*\\/)|\\b(' + KW + ")\\b|([A-Za-z_$][\\w$]*)(?=\\()|\\b(\\d+)\\b",
    "gm"
  );
  function esc(s) {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }
  function hl(src) {
    return esc(src).replace(TOKEN_RE, function (m, str, lcom, bcom, kw, fn, num) {
      var cls = str ? "tok-str" : (lcom || bcom) ? "tok-com" : kw ? "tok-kw" : fn ? "tok-fn" : "tok-num";
      return '<span class="' + cls + '">' + m + "</span>";
    });
  }

  /* ---- line texts ---- */
  var L1 = '"use strict";';
  var L2 = 'Object.defineProperty(exports,"__esModule",{value:!0});';
  var L3 = "exports.loadProfile=void 0;";
  var L4 = 'var _api=_interopRequireDefault(require("./api"));';
  var L5 = "function _interopRequireDefault(e){return e&&e.__esModule?e:{default:e}}";
  var L6 = 'function _asyncToGenerator(e){return function(){var t=this,r=arguments;return new Promise(function(n,o){var a=e.apply(t,r);function i(e){c(a,n,o,i,u,"next",e)} /* … */ i(void 0)})}}';
  var L7 = "function c(e,t,r,n,o,a,i){try{var u=e[a](i),c=u.value}catch(e){return void r(e)}u.done?t(c):Promise.resolve(c).then(n,o)}";
  var L8 = "var loadProfile=function(){";
  var L9 = "    var e=_asyncToGenerator(function*(e){";
  var L10a = '        var t=yield _api.default.fetchUser(e),r=null!=t.name?t.name:"anonymous";';
  var L10b = '        var t=yield _api.fetchUser(e),r=null!=t.name?t.name:"anonymous";';
  var L11 = "        return{name:r,avatar:null==t.profile?void 0:t.profile.avatar}";
  var L12 = "    });";
  var L13 = "    return function(t){return e.apply(this,arguments)}";
  var L14 = "}();";
  var L15 = "exports.loadProfile=loadProfile;";
  var IMP = 'import _api from "./api";';
  var L8e = "export const loadProfile=function(){";
  var A8 = "export const loadProfile = async (e)=>{";
  var A10 = '    var t = await _api.fetchUser(e),r=null!=t.name?t.name:"anonymous";';
  var A11 = "    return{name:r,avatar:null==t.profile?void 0:t.profile.avatar}";
  var A14 = "};";
  var F10 = "    const t = await _api.fetchUser(e);";
  var N2 = '    const name = t.name ?? "anonymous";';
  var F11 = "    return {";
  var N4 = "        name,";
  var N5 = "        avatar: t.profile?.avatar";
  var N6 = "    };";

  /* ---- beats: lines are [key, text, role?]
     role: "f" = focused match (blue bar, bright)
           "a" = added/rewritten by this beat (green bar, bright)
           "d" = dimmed (out of focus)
           undefined = plain bright ---- */
  var BEATS = [
    {
      status: "input · minified Babel output",
      lines: [
        ["l1", L1], ["l2", L2], ["l3", L3], ["l4", L4], ["l5", L5],
        ["h2", L6], ["h3", L7],
        ["f8", L8], ["f9", L9], ["f10", L10a], ["f11", L11], ["f12", L12],
        ["f13", L13], ["f14", L14], ["l15", L15]
      ]
    },
    {
      status: '<span class="mk">matched</span> · interop require pattern',
      lines: [
        ["l1", L1, "d"], ["l2", L2, "d"], ["l3", L3, "d"],
        ["l4", L4, "f"], ["l5", L5, "f"],
        ["h2", L6, "d"], ["h3", L7, "d"],
        ["f8", L8, "d"], ["f9", L9, "d"], ["f10", L10a, "f"], ["f11", L11, "d"],
        ["f12", L12, "d"], ["f13", L13, "d"], ["f14", L14, "d"], ["l15", L15, "d"]
      ]
    },
    {
      status: '<span class="ok">✓</span> un-interop · require("./api") → import',
      lines: [
        ["l1", L1, "d"], ["l2", L2, "d"], ["l3", L3, "d"],
        ["imp", IMP, "a"],
        ["h2", L6, "d"], ["h3", L7, "d"],
        ["f8", L8, "d"], ["f9", L9, "d"], ["f10", L10b, "a"], ["f11", L11, "d"],
        ["f12", L12, "d"], ["f13", L13, "d"], ["f14", L14, "d"], ["l15", L15, "d"]
      ]
    },
    {
      status: '<span class="mk">matched</span> · CommonJS module wrapper',
      lines: [
        ["l1", L1, "f"], ["l2", L2, "f"], ["l3", L3, "f"],
        ["imp", IMP, "d"],
        ["h2", L6, "d"], ["h3", L7, "d"],
        ["f8", L8, "f"], ["f9", L9, "d"], ["f10", L10b, "d"], ["f11", L11, "d"],
        ["f12", L12, "d"], ["f13", L13, "d"], ["f14", L14, "d"], ["l15", L15, "f"]
      ]
    },
    {
      status: '<span class="ok">✓</span> un-esm · exports.loadProfile → export const',
      lines: [
        ["imp", IMP, "d"],
        ["h2", L6, "d"], ["h3", L7, "d"],
        ["f8", L8e, "a"], ["f9", L9, "d"], ["f10", L10b, "d"], ["f11", L11, "d"],
        ["f12", L12, "d"], ["f13", L13, "d"], ["f14", L14, "d"]
      ]
    },
    {
      status: '<span class="mk">matched</span> · generator state machine',
      lines: [
        ["imp", IMP, "d"],
        ["h2", L6, "f"], ["h3", L7, "f"],
        ["f8", L8e, "f"], ["f9", L9, "f"], ["f10", L10b, "d"], ["f11", L11, "d"],
        ["f12", L12, "f"], ["f13", L13, "f"], ["f14", L14, "f"]
      ]
    },
    {
      status: '<span class="ok">✓</span> un-async-await · yield → await, trampoline removed',
      lines: [
        ["imp", IMP, "d"],
        ["f8", A8, "a"], ["f10", A10, "a"], ["f11", A11, "d"], ["f14", A14, "a"]
      ]
    },
    {
      status: '<span class="mk">matched</span> · null != / null == checks',
      lines: [
        ["imp", IMP, "d"],
        ["f8", A8, "d"], ["f10", A10, "f"], ["f11", A11, "f"], ["f14", A14, "d"]
      ]
    },
    {
      status: '<span class="ok">✓</span> un-nullish · un-optional-chaining — current CLI output',
      lines: [
        ["imp", IMP],
        ["f8", A8], ["f10", F10], ["n2", N2, "a"], ["f11", F11],
        ["n4", N4], ["n5", N5, "a"], ["n6", N6], ["f14", A14]
      ]
    }
  ];

  /* dots = stages; ▸/◂ walk individual beats */
  var STAGES = [
    { label: "minified input", beat: 0 },
    { label: "import recovery", beat: 1 },
    { label: "export recovery", beat: 3 },
    { label: "async/await recovery", beat: 5 },
    { label: "modern syntax", beat: 7 },
    { label: "final output", beat: 8 }
  ];
  var DOT_OF_BEAT = [0, 1, 1, 2, 2, 3, 3, 4, 5];

  var EXIT_MS = 460;
  var SWAP_MS = 220;
  var reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var current = 0;
  var busy = false;

  function applyRole(el, role) {
    el.classList.toggle("focus", role === "f");
    el.classList.toggle("chg", role === "a");
    el.classList.toggle("dim", role === "d");
  }

  function makeLine(key, text, role) {
    var el = document.createElement("div");
    el.className = "cl";
    el.dataset.key = key;
    el.dataset.raw = text;
    el.innerHTML = hl(text) || " ";
    applyRole(el, role);
    return el;
  }

  function renderBeat(i) {
    codeEl.textContent = "";
    BEATS[i].lines.forEach(function (l) {
      codeEl.appendChild(makeLine(l[0], l[1], l[2]));
    });
  }

  function setStatus(html) {
    if (reduceMotion) { statusEl.innerHTML = html; return; }
    statusEl.classList.add("dim");
    setTimeout(function () {
      statusEl.innerHTML = html;
      statusEl.classList.remove("dim");
    }, 180);
  }

  function swapContent(el, text) {
    el.classList.add("swap");
    setTimeout(function () {
      el.dataset.raw = text;
      el.innerHTML = hl(text) || " ";
      el.classList.remove("swap");
    }, SWAP_MS);
  }

  function transitionTo(i) {
    var beat = BEATS[i];
    var next = {};
    beat.lines.forEach(function (l) { next[l[0]] = l; });

    setStatus(beat.status);

    var nodes = Array.prototype.slice.call(codeEl.children);
    var kept = {};
    nodes.forEach(function (el) {
      if (next[el.dataset.key]) kept[el.dataset.key] = el;
      else el.classList.add("out");
    });

    setTimeout(function () {
      nodes.forEach(function (el) {
        if (!kept[el.dataset.key]) el.remove();
      });
      var ref = null;
      for (var j = beat.lines.length - 1; j >= 0; j--) {
        var l = beat.lines[j];
        var el = kept[l[0]];
        if (el) {
          applyRole(el, l[2]);
          if (el.dataset.raw !== l[1]) swapContent(el, l[1]);
          if (el.nextSibling !== ref) codeEl.insertBefore(el, ref);
        } else {
          el = makeLine(l[0], l[1], l[2]);
          el.classList.add("in");
          codeEl.insertBefore(el, ref);
          void el.offsetHeight; /* force reflow so the transition runs */
          el.classList.remove("in");
        }
        ref = el;
      }
    }, EXIT_MS);
  }

  var dots = STAGES.map(function (stage, i) {
    var dot = document.createElement("button");
    dot.type = "button";
    dot.className = "dot";
    dot.setAttribute("aria-label", "Stage " + (i + 1) + ": " + stage.label);
    dot.addEventListener("click", function () { goTo(stage.beat); });
    dotsEl.appendChild(dot);
    return dot;
  });

  function updateControls() {
    prevBtn.disabled = current === 0;
    nextBtn.disabled = current === BEATS.length - 1;
    nextBtn.classList.toggle("pulse", current === 0 && !reduceMotion);
    var active = DOT_OF_BEAT[current];
    dots.forEach(function (dot, i) {
      dot.classList.toggle("active", i === active);
    });
  }

  function goTo(i) {
    if (busy || i < 0 || i >= BEATS.length || i === current) return;
    current = i;
    updateControls();
    if (reduceMotion) {
      renderBeat(i);
      statusEl.innerHTML = BEATS[i].status;
      return;
    }
    busy = true;
    transitionTo(i);
    setTimeout(function () { busy = false; }, EXIT_MS + SWAP_MS + 120);
  }

  prevBtn.addEventListener("click", function () { goTo(current - 1); });
  nextBtn.addEventListener("click", function () { goTo(current + 1); });
  codeEl.closest(".demo").addEventListener("keydown", function (e) {
    if (e.key === "ArrowRight") { goTo(current + 1); e.preventDefault(); }
    if (e.key === "ArrowLeft") { goTo(current - 1); e.preventDefault(); }
  });

  renderBeat(0);
  statusEl.innerHTML = BEATS[0].status;
  updateControls();
})();
