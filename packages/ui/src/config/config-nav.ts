export interface ConfigNavSub {
  id: string;
  label: string;
}

export interface ConfigNavGroup {
  id: string;
  label: string;
  icon: string; // glyphe/emoji rendu tel quel
  accent: string; // couleur CSS de l'accent actif
  sub?: ConfigNavSub[];
}
