/* Audited initial Web IDL subset: Document.body/title/getElementById,
   Node.textContent, and supported CSSStyleDeclaration properties. The call gate
   is captured and removed before page execution. No browser-site-specific data. */
(() => {
  'use strict';
  const call = globalThis.__rustosHost;
  delete globalThis.__rustosHost;
  const nodes = new Map();
  const ids = new WeakMap();
  const styles = new WeakMap();
  const token = {};
  class Element {
    constructor(secret, id) {
      if (secret !== token) throw new TypeError('Illegal constructor');
      ids.set(this, id);
    }
    get textContent() { return call(4, id(this), '', ''); }
    set textContent(value) { call(5, id(this), value == null ? '' : String(value), ''); }
    get style() {
      if (!styles.has(this)) {
        const owner = id(this);
        const style = {};
        for (const [name, css] of [
          ['color','color'], ['backgroundColor','background-color'],
          ['display','display'], ['fontSize','font-size'],
          ['fontFamily','font-family'], ['fontWeight','font-weight'],
          ['width','width'], ['height','height']
        ]) {
          Object.defineProperty(style, name, {
            enumerable: true,
            get() { return call(6, owner, css, ''); },
            set(value) { call(7, owner, css, String(value)); }
          });
        }
        styles.set(this, style);
      }
      return styles.get(this);
    }
  }
  function id(element) {
    if (!ids.has(element)) throw new TypeError('Illegal invocation');
    return ids.get(element);
  }
  function wrap(node) {
    if (node === null) return null;
    if (!nodes.has(node)) nodes.set(node, new Element(token, node));
    return nodes.get(node);
  }
  const document = {
    get body() { return wrap(call(1, 0, '', '')); },
    get title() { return call(2, 0, '', ''); },
    set title(value) { call(3, 0, String(value), ''); },
    getElementById(value) { return wrap(call(0, 0, String(value), '')); }
  };
  Object.defineProperty(globalThis, 'document', {value:document});
  Object.defineProperty(globalThis, 'Element', {value:Element});
  globalThis.window = globalThis;
  globalThis.self = globalThis;
})();
