# P4: DOM attributes and the Google results gate

P4 remains **in progress**. Google is still the default start page. This increment
adds general attribute mutations and browser base64 utilities to the restricted
native document worker. It does not pass the Google results acceptance gate.

## Ownership and supported behavior

The controller, parser, resource loader and script worker remain Ring 3 programs.
No kernel, syscall, toolchain or dependency changes are needed. The worker owns a
fresh document and realm; only bounded, pointer-free effects cross back to jbrowse.

- `Element.getAttribute`, `hasAttribute`, `setAttribute`, `removeAttribute`, `id`
  and `className` operate on the same stable element wrappers as `textContent`.
  HTML attribute lookup folds ASCII case. Names use a bounded ASCII subset;
  namespaces and complete XML/Unicode name validation are not implemented.
- Attribute changes update the native node. Re-layout observes class, style,
  link and control changes. Recollection updates successful form controls, and a
  rejected form/attribute budget restores the previous attributes before returning
  an error. Batch application still validates on a copy before publication.
- `atob`/`btoa` use binary strings, not UTF-8 conversion. Decoding accepts ASCII
  whitespace and omitted padding; invalid alphabets/padding and non-Latin1 encoding
  inputs throw an error named `InvalidCharacterError`. A full `DOMException`
  interface is not supplied. Both input and output are capped at8192 code units,
  within the existing engine memory/CPU limits.

The existing `JDM1` wire adds kind3 (set attribute) and kind4 (remove attribute).
Both contain node ID and a nonempty name; kind4 must have an empty value. Existing
kind0/1/2 meanings and all count/length/reserved/truncation rules remain unchanged.
Old decoders reject these new effects; rebuild controller and worker together.
No native pointer or OS authority is transferred. Limits remain256 effects,
8192 wire bytes,64 attributes per node and128 bytes per attribute name.

Changing an `on*` attribute does not create an event handler. Dynamic script loading,
events, timers, location/navigation bindings, cookies and external/module scripts
remain unsupported. Inline stylesheet text mutations do not rebuild stylesheet
rules. This is a declared subset, not WPT conformance.

## Public-site diagnosis and next work

On2026-11-06, an anonymous host request to Google's normal search endpoint with
`RustOS-curl/1`, HTTP/1.1 and identity encoding returned a roughly93KiB JavaScript
challenge page. The served code uses attribute mutation and base64 utilities,
alongside missing events, clocks, DOM construction, navigation and session APIs.
The host fetch is diagnostic evidence, not a native results pass. No challenge
source, tokens, credentials or provider-specific code is installed in the browser.

Implement the remaining APIs with independent fixtures and maintain strict
results rejection. P4 exits only after three real queries show readable titles,
snippets and destinations, following a result, Back and refinement. CAPTCHA or
service rejection remains a failure; these bindings do not promise acceptance.
See the [roadmap](browser-support-plan.md#p4--google-results-as-the-first-modern-site-gate).

## Validation

Executed red/green evidence, screenshots and reproduction commands are recorded in
[the P4 evidence index](browser-p4/README.md). No manual host GUI check or broad WPT
run is claimed. API reference targets checked2026-11-06:
[DOM attribute methods](https://dom.spec.whatwg.org/#dom-element-setattribute) and
[HTML base64 utilities](https://html.spec.whatwg.org/multipage/webappapis.html#atob).
