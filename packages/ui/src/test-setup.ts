// jsdom n'implémente pas `Element.prototype.scrollIntoView` (gap connu,
// documenté par jsdom lui-même) — `@nuxt/ui`'s `ChatMessages` l'appelle pour
// faire défiler vers le dernier message envoyé. Sans ce polyfill : rejection
// non gérée à chaque test montant `ChatSession`/`Chat`, qui fait échouer la
// run malgré des tests individuellement verts.
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}
