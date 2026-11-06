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
    getAttribute(name) { return call(8, id(this), String(name).toLowerCase(), ''); }
    hasAttribute(name) { return this.getAttribute(name) !== null; }
    setAttribute(name, value) {
      name = String(name);
      // Bounded ASCII HTML attribute-name profile; no namespaces/XML DOM yet.
      if (!/^[A-Za-z0-9_:.-]+$/.test(name)) {
        const error = new Error('Invalid attribute name');
        error.name = 'InvalidCharacterError'; throw error;
      }
      call(9, id(this), name.toLowerCase(), String(value));
    }
    removeAttribute(name) { call(10, id(this), String(name).toLowerCase(), ''); }
    get id() { return this.getAttribute('id') || ''; }
    set id(value) { this.setAttribute('id', value); }
    get className() { return this.getAttribute('class') || ''; }
    set className(value) { this.setAttribute('class', value); }
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
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  function invalidBase64() {
    const error = new Error('Invalid base64 binary string');
    error.name = 'InvalidCharacterError'; throw error;
  }
  function bounded(value) {
    value = String(value);
    if (value.length > 8192) throw new RangeError('Base64 resource limit');
    return value;
  }
  globalThis.btoa = function(value) {
    value = bounded(value);
    if (4 * Math.ceil(value.length / 3) > 8192) throw new RangeError('Base64 resource limit');
    let result = '';
    for (let i=0; i<value.length; i+=3) {
      const a=value.charCodeAt(i), b=i+1<value.length?value.charCodeAt(i+1):0,
            c=i+2<value.length?value.charCodeAt(i+2):0;
      if (a>255 || b>255 || c>255) invalidBase64();
      result += alphabet[a>>2] + alphabet[((a&3)<<4)|(b>>4)]
        + (i+1<value.length?alphabet[((b&15)<<2)|(c>>6)]:'=')
        + (i+2<value.length?alphabet[c&63]:'=');
    }
    return result;
  };
  globalThis.atob = function(value) {
    value = bounded(value).replace(/[\t\n\f\r ]/g, '');
    if (value.length%4===0) value=value.replace(/==?$/, '');
    if (value.length%4===1 || /[^A-Za-z0-9+/]/.test(value)) invalidBase64();
    let result='', bits=0, count=0;
    for (let i=0; i<value.length; i++) {
      bits=(bits<<6)|alphabet.indexOf(value[i]); count+=6;
      if(count>=8) {count-=8;result+=String.fromCharCode((bits>>count)&255);bits&=(1<<count)-1;}
    }
    return result;
  };
})();
