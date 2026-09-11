/* tui docs — shared behaviour for the index and both app pages.
   Everything here is progressive: with scripting off the pages still read,
   the commands are still selectable, and nothing is left hidden. */
(function () {
  'use strict';

  var calm = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  /* ---- copy the install command ------------------------------------- */

  function legacyCopy(text) {
    var field = document.createElement('textarea');
    field.value = text;
    field.setAttribute('readonly', '');
    field.style.cssText = 'position:fixed;top:-9999px;opacity:0';
    document.body.appendChild(field);
    field.select();
    var ok = false;
    try { ok = document.execCommand('copy'); } catch (e) { ok = false; }
    document.body.removeChild(field);
    return ok;
  }

  function copy(text) {
    if (navigator.clipboard && window.isSecureContext) {
      // Safari can leave the clipboard promise pending when the gesture is
      // not trusted, so fall back rather than hang on the button.
      var timeout = new Promise(function (_, reject) {
        setTimeout(function () { reject(new Error('timed out')); }, 1200);
      });
      return Promise.race([navigator.clipboard.writeText(text), timeout])
        .then(function () { return true; })
        .catch(function () { return legacyCopy(text); });
    }
    return Promise.resolve(legacyCopy(text));
  }

  Array.prototype.forEach.call(document.querySelectorAll('.cmd button'), function (button) {
    button.addEventListener('click', function () {
      copy(button.dataset.cmd).then(function (ok) {
        button.textContent = ok ? 'copied' : 'press ⌘C';
        button.classList.toggle('done', ok);
        setTimeout(function () {
          button.textContent = 'copy';
          button.classList.remove('done');
        }, 1800);
      });
    });
  });

  /* ---- type the opening command ------------------------------------- */

  var typed = document.querySelector('[data-type]');
  if (typed && !calm) {
    var text = typed.textContent;
    typed.textContent = '';
    var at = 0;
    (function step() {
      typed.textContent = text.slice(0, ++at);
      if (at < text.length) setTimeout(step, 34);
    })();
  }

  /* ---- play the recorded demos -------------------------------------- */

  /* Each demo ships as a poster PNG with the GIF named on the img. The GIF is
     only fetched once the frame is on screen, and never when the reader has
     asked for less motion — a GIF cannot be stopped, so the button is the only
     way back out of one. */

  Array.prototype.forEach.call(document.querySelectorAll('.demo'), function (frame) {
    var img = frame.querySelector('img[data-demo]');
    var button = frame.querySelector('.pp');
    if (!img || !button) return;

    var poster = img.getAttribute('src');
    var running = false;

    function play() {
      img.src = img.getAttribute('data-demo');
      running = true;
      button.textContent = 'pause';
    }

    function pause() {
      img.src = poster;
      running = false;
      button.textContent = 'play';
    }

    button.hidden = false;
    button.textContent = 'play';
    button.addEventListener('click', function () {
      if (running) pause(); else play();
    });

    if (calm) return;
    if (!('IntersectionObserver' in window)) { play(); return; }

    var near = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (!entry.isIntersecting) return;
        play();
        near.unobserve(entry.target);
      });
    }, { rootMargin: '0px 0px -10% 0px' });

    near.observe(frame);

    // Belt and braces, as with the reveal below: if the observer never
    // reports, start the one demo nearest the top rather than leave every
    // frame sitting on its poster with no sign that it moves.
    setTimeout(function () {
      if (!running && frame.getBoundingClientRect().top < window.innerHeight) {
        near.unobserve(frame);
        play();
      }
    }, 1500);
  });

  /* ---- reveal blocks as they arrive --------------------------------- */

  var main = document.querySelector('main');
  if (main && !calm && 'IntersectionObserver' in window) {
    var blocks = Array.prototype.filter.call(main.children, function (el) {
      return el.tagName !== 'SCRIPT';
    });
    blocks.forEach(function (el) { el.classList.add('rv'); });

    var seen = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (!entry.isIntersecting) return;
        entry.target.classList.add('in');
        seen.unobserve(entry.target);
      });
    }, { rootMargin: '0px 0px -8% 0px' });

    blocks.forEach(function (el) { seen.observe(el); });

    // Belt and braces: if the observer never reports, show everything anyway
    // rather than leave the page blank.
    setTimeout(function () {
      blocks.forEach(function (el) { el.classList.add('in'); });
    }, 1500);
  }

  /* ---- reading progress --------------------------------------------- */

  if (!calm) {
    var bar = document.createElement('div');
    bar.className = 'prog';
    bar.setAttribute('aria-hidden', 'true');
    document.body.appendChild(bar);

    var queued = false;
    function draw() {
      queued = false;
      var span = document.documentElement.scrollHeight - window.innerHeight;
      var ratio = span > 0 ? window.scrollY / span : 0;
      bar.style.width = Math.max(0, Math.min(1, ratio)) * 100 + '%';
    }
    window.addEventListener('scroll', function () {
      if (queued) return;
      queued = true;
      window.requestAnimationFrame(draw);
    }, { passive: true });
    draw();
  }
})();
