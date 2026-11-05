// Blanks the contents of `--` comments and of string and backtick literals,
// keeping the delimiters, line breaks and every offset. Free of other
// imports so the standalone formatter can use it.
export function maskCommentsAndStrings(text: string): string {
  const out = text.split("");
  const n = text.length;
  const blank = (i: number): void => {
    if (text[i] !== "\n") {
      out[i] = " ";
    }
  };
  let i = 0;
  while (i < n) {
    const ch = text[i];
    if (ch === "-" && text[i + 1] === "-") {
      i += 2; // keep the `--`
      while (i < n && text[i] !== "\n") {
        blank(i);
        i += 1;
      }
    } else if (ch === '"') {
      i += 1; // keep the opening quote
      while (i < n && text[i] !== '"' && text[i] !== "\n") {
        if (text[i] === "\\") {
          blank(i);
          if (i + 1 < n) {
            blank(i + 1);
            i += 2;
            continue;
          }
        }
        blank(i);
        i += 1;
      }
      if (i < n && text[i] === '"') {
        i += 1; // keep the closing quote
      }
    } else if (ch === "`") {
      i += 1; // keep the opening backtick
      while (i < n && text[i] !== "`" && text[i] !== "\n" && text[i] !== "\r") {
        blank(i);
        i += 1;
      }
      if (i < n && text[i] === "`") {
        i += 1; // keep the closing backtick
      }
    } else {
      i += 1;
    }
  }
  return out.join("");
}
