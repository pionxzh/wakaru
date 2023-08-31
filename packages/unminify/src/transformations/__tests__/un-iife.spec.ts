import transform from '../un-iife'
import { defineInlineTest } from './test-utils'

const inlineTest = defineInlineTest(transform)

inlineTest('iife with arguments',
  `
(function(i, s, o, g, r, a, m) {
  i['GoogleAnalyticsObject'] = r;
  i[r] = i[r] || function() { (i[r].q = i[r].q||[]).push(arguments) }
  i[r].l = 1 * new Date();
  a = s.createElement(o);
  m = s.getElementsByTagName(o)[0];
  a.async = 1;
  a.src = g;
  m.parentNode.insertBefore(a, m);
})(window, document, 'script', 'https://www.google-analytics.com/analytics.js', 'ga');
`,
  `
(function(window, document, o, g, r, a, m) {
  window['GoogleAnalyticsObject'] = r;
  window[r] = window[r] || function() { (window[r].q = window[r].q||[]).push(arguments) }
  window[r].l = 1 * new Date();
  a = document.createElement(o);
  m = document.getElementsByTagName(o)[0];
  a.async = 1;
  a.src = g;
  m.parentNode.insertBefore(a, m);
})(window, document, 'script', 'https://www.google-analytics.com/analytics.js', 'ga');
`,
)

inlineTest('iife without arguments',
  `
(function(i, s, o, g, r, a, m) {
  i['GoogleAnalyticsObject'] = r;
  // i[r] = i[r] || function() { (i[r].q = i[r].q||[]).push(arguments) }
  i[r].l = 1 * new Date();
  a = s.createElement(o);
  m = s.getElementsByTagName(o)[0];
  a.async = 1;
  a.src = g;
  m.parentNode.insertBefore(a, m);
})(window, document, 'script', 'https://www.google-analytics.com/analytics.js', 'ga');
`,
  `
(function(window, document, a, m) {
  const o = 'script';
  const g = 'https://www.google-analytics.com/analytics.js';
  const r = 'ga';
  window['GoogleAnalyticsObject'] = r;
  // i[r] = i[r] || function() { (i[r].q = i[r].q||[]).push(arguments) }
  window[r].l = 1 * new Date();
  a = document.createElement(o);
  m = document.getElementsByTagName(o)[0];
  a.async = 1;
  a.src = g;
  m.parentNode.insertBefore(a, m);
})(window, document);
`,
)

inlineTest('iife param with longer name should not be renamed',
  `
(function(win, s, a) {
  win['GoogleAnalyticsObject'] = 'ga';
  a = s.createElement('script');
  a.src = 'url';
})(window, document);
`,
  `
(function(win, document, a) {
  win['GoogleAnalyticsObject'] = 'ga';
  a = document.createElement('script');
  a.src = 'url';
})(window, document);
`,
)

inlineTest('iife argument with shorter name should not be renamed',
  `
(function(i, s, a) {
  i['GoogleAnalyticsObject'] = 'ga';
  a = s.createElement('script');
  a.src = 'url';
})(w, document);
`,
  `
(function(i, document, a) {
  i['GoogleAnalyticsObject'] = 'ga';
  a = document.createElement('script');
  a.src = 'url';
})(w, document);
`,
)
