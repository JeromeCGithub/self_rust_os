class Counter { #n=40; next(){return ++this.#n} }
const counter=new Counter();
JSON.stringify([counter.next(),counter.next(),[1,2,3].map(x=>x*2),/rust/i.test('Rust'),new Uint8Array([7,8])[1],'café'.normalize('NFC')]);
