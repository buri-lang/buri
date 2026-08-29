//! The colour schemes, and the one stylesheet the whole site shares.
//!
//! Every hex below is a published value read off the scheme's own source —
//! `catppuccin/palette`, the Dracula specification, nordtheme.com's palette
//! documentation, `morhetz/gruvbox`, `atom/one-dark-syntax`, Ethan
//! Schoonover's Solarized values table, and `folke/tokyonight.nvim`. Which
//! palette entry fills which role follows each scheme's own editor port, so a
//! reader who knows a theme from their editor recognises it here.
//!
//! The scheme colours the page as well as the code. A site that themes only
//! its snippets is a site with one theme and a costume.

/// One scheme. Every field is a colour the scheme publishes; nothing here is
/// mixed, lightened, or guessed.
pub struct Theme {
    /// What `data-theme` holds, and what is written to `localStorage`.
    pub id: &'static str,
    pub name: &'static str,
    /// The page, and the browser's own form controls and scrollbars with it.
    pub scheme: &'static str,
    pub page: &'static str,
    pub panel: &'static str,
    pub raised: &'static str,
    pub border: &'static str,
    pub text: &'static str,
    /// Headings and anything else that carries weight.
    pub strong: &'static str,
    pub muted: &'static str,
    pub accent: &'static str,
    pub accent_hover: &'static str,
    pub selection: &'static str,
    pub code_background: &'static str,
    pub code_text: &'static str,
    pub keyword: &'static str,
    pub string: &'static str,
    pub number: &'static str,
    pub comment: &'static str,
    pub operator: &'static str,
    pub identifier: &'static str,
    pub type_name: &'static str,
}

/// The scheme a page is served with before `localStorage` has an opinion.
pub const DEFAULT: &str = "catppuccin-mocha";

pub const THEMES: &[Theme] = &[
    // catppuccin/palette `palette.json`; roles from catppuccin/vscode.
    Theme {
        id: "catppuccin-mocha",
        name: "Catppuccin Mocha",
        scheme: "dark",
        page: "#1e1e2e",
        panel: "#181825",
        raised: "#313244",
        border: "#45475a",
        text: "#cdd6f4",
        strong: "#cdd6f4",
        muted: "#a6adc8",
        accent: "#89b4fa",
        accent_hover: "#b4befe",
        selection: "#45475a",
        code_background: "#181825",
        code_text: "#cdd6f4",
        keyword: "#cba6f7",
        string: "#a6e3a1",
        number: "#fab387",
        comment: "#9399b2",
        operator: "#94e2d5",
        identifier: "#89b4fa",
        type_name: "#f9e2af",
    },
    Theme {
        id: "catppuccin-latte",
        name: "Catppuccin Latte",
        scheme: "light",
        page: "#eff1f5",
        panel: "#e6e9ef",
        raised: "#ccd0da",
        border: "#bcc0cc",
        text: "#4c4f69",
        strong: "#4c4f69",
        muted: "#6c6f85",
        accent: "#1e66f5",
        accent_hover: "#7287fd",
        selection: "#ccd0da",
        code_background: "#e6e9ef",
        code_text: "#4c4f69",
        keyword: "#8839ef",
        string: "#40a02b",
        number: "#fe640b",
        comment: "#7c7f93",
        operator: "#179299",
        identifier: "#1e66f5",
        type_name: "#df8e1d",
    },
    // spec.draculatheme.com, with the UI shades from dracula/visual-studio-code.
    Theme {
        id: "dracula",
        name: "Dracula",
        scheme: "dark",
        page: "#282a36",
        panel: "#21222c",
        raised: "#343746",
        border: "#191a21",
        text: "#f8f8f2",
        strong: "#f8f8f2",
        muted: "#6272a4",
        accent: "#bd93f9",
        accent_hover: "#ff79c6",
        selection: "#44475a",
        code_background: "#21222c",
        code_text: "#f8f8f2",
        keyword: "#ff79c6",
        string: "#f1fa8c",
        number: "#bd93f9",
        comment: "#6272a4",
        operator: "#ff79c6",
        identifier: "#50fa7b",
        type_name: "#8be9fd",
    },
    // nordtheme.com/docs/colors-and-palettes, with its own role assignments.
    Theme {
        id: "nord",
        name: "Nord",
        scheme: "dark",
        page: "#2e3440",
        panel: "#3b4252",
        raised: "#3b4252",
        border: "#434c5e",
        text: "#d8dee9",
        strong: "#eceff4",
        muted: "#d8dee9",
        accent: "#88c0d0",
        accent_hover: "#8fbcbb",
        selection: "#434c5e",
        code_background: "#2e3440",
        code_text: "#d8dee9",
        keyword: "#81a1c1",
        string: "#a3be8c",
        number: "#b48ead",
        comment: "#4c566a",
        operator: "#81a1c1",
        identifier: "#88c0d0",
        type_name: "#8fbcbb",
    },
    // morhetz/gruvbox `colors/gruvbox.vim`; dark mode uses the bright set.
    Theme {
        id: "gruvbox-dark",
        name: "Gruvbox Dark",
        scheme: "dark",
        page: "#282828",
        panel: "#1d2021",
        raised: "#3c3836",
        border: "#504945",
        text: "#ebdbb2",
        strong: "#fbf1c7",
        muted: "#a89984",
        accent: "#83a598",
        accent_hover: "#8ec07c",
        selection: "#504945",
        code_background: "#1d2021",
        code_text: "#ebdbb2",
        keyword: "#fb4934",
        string: "#b8bb26",
        number: "#d3869b",
        comment: "#928374",
        operator: "#ebdbb2",
        identifier: "#83a598",
        type_name: "#fabd2f",
    },
    // atom/one-dark-syntax `styles/colors.less`, with one-dark-ui's shades.
    Theme {
        id: "one-dark",
        name: "One Dark",
        scheme: "dark",
        page: "#282c34",
        panel: "#21252b",
        raised: "#3e4451",
        border: "#181a1f",
        text: "#abb2bf",
        strong: "#abb2bf",
        muted: "#828997",
        accent: "#61afef",
        accent_hover: "#56b6c2",
        selection: "#3e4451",
        code_background: "#21252b",
        code_text: "#abb2bf",
        keyword: "#c678dd",
        string: "#98c379",
        number: "#d19a66",
        comment: "#5c6370",
        operator: "#abb2bf",
        identifier: "#61afef",
        type_name: "#e5c07b",
    },
    // altercation/solarized: base3 background, base00 body text, base1
    // comments — the light assignment the README's own mixin makes.
    Theme {
        id: "solarized-light",
        name: "Solarized Light",
        scheme: "light",
        page: "#fdf6e3",
        panel: "#eee8d5",
        raised: "#eee8d5",
        border: "#93a1a1",
        text: "#657b83",
        strong: "#586e75",
        muted: "#93a1a1",
        accent: "#268bd2",
        accent_hover: "#6c71c4",
        selection: "#eee8d5",
        code_background: "#eee8d5",
        code_text: "#657b83",
        keyword: "#859900",
        string: "#2aa198",
        number: "#2aa198",
        comment: "#93a1a1",
        operator: "#859900",
        identifier: "#268bd2",
        type_name: "#b58900",
    },
    // folke/tokyonight.nvim, the `night` variant.
    Theme {
        id: "tokyo-night",
        name: "Tokyo Night",
        scheme: "dark",
        page: "#1a1b26",
        panel: "#16161e",
        raised: "#292e42",
        border: "#3b4261",
        text: "#c0caf5",
        strong: "#c0caf5",
        muted: "#a9b1d6",
        accent: "#7aa2f7",
        accent_hover: "#7dcfff",
        selection: "#292e42",
        code_background: "#16161e",
        code_text: "#c0caf5",
        keyword: "#7dcfff",
        string: "#9ece6a",
        number: "#ff9e64",
        comment: "#565f89",
        operator: "#89ddff",
        identifier: "#7aa2f7",
        type_name: "#2ac3de",
    },
];

pub fn find(id: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|theme| theme.id == id)
}

/// Every theme's custom properties, keyed by `data-theme`.
///
/// A theme is a block of variables and nothing else — no theme has a rule of
/// its own — so adding one cannot change the layout of a page.
fn palettes() -> String {
    let mut out = String::new();
    for theme in THEMES {
        out.push_str(&format!(
            "[data-theme=\"{id}\"] {{\n  \
             color-scheme: {scheme};\n  \
             --page: {page};\n  \
             --panel: {panel};\n  \
             --raised: {raised};\n  \
             --border: {border};\n  \
             --text: {text};\n  \
             --strong: {strong};\n  \
             --muted: {muted};\n  \
             --accent: {accent};\n  \
             --accent-hover: {accent_hover};\n  \
             --selection: {selection};\n  \
             --code-page: {code_background};\n  \
             --code-text: {code_text};\n  \
             --code-keyword: {keyword};\n  \
             --code-string: {string};\n  \
             --code-number: {number};\n  \
             --code-comment: {comment};\n  \
             --code-operator: {operator};\n  \
             --code-identifier: {identifier};\n  \
             --code-type: {type_name};\n\
             }}\n\n",
            id = theme.id,
            scheme = theme.scheme,
            page = theme.page,
            panel = theme.panel,
            raised = theme.raised,
            border = theme.border,
            text = theme.text,
            strong = theme.strong,
            muted = theme.muted,
            accent = theme.accent,
            accent_hover = theme.accent_hover,
            selection = theme.selection,
            code_background = theme.code_background,
            code_text = theme.code_text,
            keyword = theme.keyword,
            string = theme.string,
            number = theme.number,
            comment = theme.comment,
            operator = theme.operator,
            identifier = theme.identifier,
            type_name = theme.type_name,
        ));
    }
    out
}

/// The whole stylesheet: the palettes, then the layout that reads them.
pub fn stylesheet() -> String {
    let mut out = String::with_capacity(16 * 1024);
    out.push_str(&palettes());
    out.push_str(LAYOUT);
    out
}

/// Hand-written, no framework. The measure is set on the article rather than
/// on the page, so a wide table can still overflow its own scroller without
/// dragging the prose out with it.
const LAYOUT: &str = r#"*, *::before, *::after { box-sizing: border-box; }

html {
  background: var(--page);
  -webkit-text-size-adjust: 100%;
}

body {
  margin: 0;
  background: var(--page);
  color: var(--text);
  font-family: ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
  font-size: 17px;
  line-height: 1.65;
}

::selection { background: var(--selection); }

code, pre, kbd, samp {
  font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace;
  font-size: 0.875em;
}

a { color: var(--accent); text-decoration-thickness: 1px; text-underline-offset: 2px; }
a:hover { color: var(--accent-hover); }
a:focus-visible, select:focus-visible, summary:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
  border-radius: 3px;
}

.skip {
  position: absolute;
  left: -9999px;
  top: 0;
  padding: 0.5rem 1rem;
  background: var(--raised);
  z-index: 10;
}
.skip:focus { left: 0; }

/* -- the masthead -------------------------------------------------------- */

.masthead {
  position: sticky;
  top: 0;
  z-index: 5;
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 0.5rem 1.25rem;
  padding: 0.6rem 1.5rem;
  background: var(--panel);
  border-bottom: 1px solid var(--border);
}

.wordmark {
  font-weight: 700;
  font-size: 1.1rem;
  letter-spacing: 0.02em;
  color: var(--strong);
  text-decoration: none;
}

.masthead nav { display: flex; flex-wrap: wrap; gap: 0 1rem; }
.masthead nav a { color: var(--muted); text-decoration: none; font-size: 0.92rem; }
.masthead nav a:hover, .masthead nav a[aria-current="true"] { color: var(--accent); }

.theme { margin-left: auto; display: flex; align-items: center; gap: 0.5rem; }
.theme label { color: var(--muted); font-size: 0.85rem; }
.theme select {
  background: var(--raised);
  color: var(--text);
  border: 1px solid var(--border);
  border-radius: 5px;
  padding: 0.25rem 0.5rem;
  font: inherit;
  font-size: 0.85rem;
}

/* -- the frame ----------------------------------------------------------- */

.frame {
  display: grid;
  grid-template-columns: 17rem minmax(0, 1fr);
  gap: 2.5rem;
  max-width: 82rem;
  margin: 0 auto;
  padding: 2rem 1.5rem 5rem;
}

.sidebar { font-size: 0.92rem; }
.sidebar > * + * { margin-top: 1.5rem; }
.sidebar h2 {
  font-size: 0.75rem;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  color: var(--muted);
  margin: 0 0 0.5rem;
}
.sidebar ul { list-style: none; margin: 0; padding: 0; }
.sidebar li { margin: 0.15rem 0; }
.sidebar a {
  display: block;
  padding: 0.15rem 0.6rem;
  border-left: 2px solid transparent;
  color: var(--text);
  text-decoration: none;
}
.sidebar a:hover { color: var(--accent); }
.sidebar a[aria-current="page"] {
  color: var(--accent);
  border-left-color: var(--accent);
  background: var(--raised);
}

main { min-width: 0; }
article { max-width: 46rem; }

/* -- prose --------------------------------------------------------------- */

article h1, article h2, article h3, article h4, article h5, article h6 {
  color: var(--strong);
  line-height: 1.25;
  margin: 2.2rem 0 0.8rem;
  scroll-margin-top: 4.5rem;
}
article > :first-child { margin-top: 0; }
article h1 { font-size: 2rem; letter-spacing: -0.01em; }
article h2 { font-size: 1.45rem; }
article h3 { font-size: 1.15rem; }
article h4, article h5, article h6 { font-size: 1rem; }

.anchor {
  margin-left: 0.4rem;
  color: var(--muted);
  text-decoration: none;
  opacity: 0;
  font-weight: 400;
}
h1:hover .anchor, h2:hover .anchor, h3:hover .anchor,
h4:hover .anchor, h5:hover .anchor, h6:hover .anchor,
.anchor:focus-visible { opacity: 1; }

article p, article ul, article ol, article blockquote { margin: 0 0 1.1rem; }
article li { margin: 0.3rem 0; }
article li > ul, article li > ol { margin: 0.3rem 0 0.3rem; }

article :not(pre) > code {
  background: var(--raised);
  border-radius: 4px;
  padding: 0.12em 0.35em;
}

blockquote {
  border-left: 3px solid var(--border);
  padding-left: 1rem;
  color: var(--muted);
}

hr { border: 0; border-top: 1px solid var(--border); margin: 2rem 0; }

pre {
  background: var(--code-page);
  color: var(--code-text);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 0.9rem 1.1rem;
  margin: 0 0 1.2rem;
  overflow-x: auto;
  line-height: 1.55;
}
pre code { background: none; padding: 0; }

pre .keyword { color: var(--code-keyword); }
pre .string { color: var(--code-string); }
pre .number { color: var(--code-number); }
pre .comment { color: var(--code-comment); font-style: italic; }
pre .operator { color: var(--code-operator); }
pre .identifier { color: var(--code-identifier); }
pre .type { color: var(--code-type); }

.table-scroll { overflow-x: auto; margin: 0 0 1.2rem; }
table { border-collapse: collapse; width: 100%; font-size: 0.94rem; }
th, td { border: 1px solid var(--border); padding: 0.45rem 0.7rem; text-align: left; vertical-align: top; }
th { background: var(--raised); color: var(--strong); }
.align-right { text-align: right; }
.align-center { text-align: center; }

/* -- page furniture ------------------------------------------------------ */

.crumbs { color: var(--muted); font-size: 0.85rem; margin: 0 0 0.6rem; }
.crumbs a { color: var(--muted); }

.facts {
  margin: 0 0 1.6rem;
  padding: 0.9rem 1.1rem;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 8px;
  display: grid;
  grid-template-columns: max-content minmax(0, 1fr);
  gap: 0.3rem 1rem;
  font-size: 0.94rem;
}
.facts dt { color: var(--muted); }
.facts dd { margin: 0; }

.listing { list-style: none; margin: 0; padding: 0; }
.listing li { margin: 0 0 1rem; }
.listing a { font-weight: 600; text-decoration: none; }
.listing a:hover { text-decoration: underline; }
.listing code { color: var(--muted); margin-left: 0.5rem; font-size: 0.85em; }
.listing p { margin: 0.1rem 0 0; color: var(--muted); font-size: 0.94rem; }

.also { margin: 2.5rem 0 0; }
.also h2 { font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.08em; color: var(--muted); }

.colophon {
  margin-top: 3rem;
  padding-top: 1.2rem;
  border-top: 1px solid var(--border);
  color: var(--muted);
  font-size: 0.9rem;
  display: flex;
  flex-wrap: wrap;
  gap: 0.5rem 1.5rem;
}

.adapted { color: var(--muted); font-size: 0.85rem; margin-top: 2rem; }

@media (max-width: 60rem) {
  .frame { grid-template-columns: minmax(0, 1fr); gap: 1.5rem; }
  .sidebar { order: 2; border-top: 1px solid var(--border); padding-top: 1.5rem; }
  .theme { margin-left: 0; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_enough_schemes_and_the_default_is_one_of_them() {
        assert!(THEMES.len() >= 6 && THEMES.len() <= 10, "{} schemes", THEMES.len());
        assert!(find(DEFAULT).is_some());
        assert!(find("catppuccin-mocha").is_some(), "the one the maintainer asked for by name");
    }

    /// A colour written as anything but a six-digit hex is a colour somebody
    /// mixed rather than read off the scheme.
    #[test]
    fn every_colour_is_a_published_hex() {
        for theme in THEMES {
            for colour in [
                theme.page,
                theme.panel,
                theme.raised,
                theme.border,
                theme.text,
                theme.strong,
                theme.muted,
                theme.accent,
                theme.accent_hover,
                theme.selection,
                theme.code_background,
                theme.code_text,
                theme.keyword,
                theme.string,
                theme.number,
                theme.comment,
                theme.operator,
                theme.identifier,
                theme.type_name,
            ] {
                assert!(
                    colour.len() == 7
                        && colour.starts_with('#')
                        && colour.get(1..).is_some_and(|rest| {
                            rest.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                        }),
                    "`{}` carries `{colour}`, which is not a lowercase six-digit hex",
                    theme.id
                );
            }
        }
    }

    #[test]
    fn every_identifier_is_unique_and_url_safe() {
        let mut seen: Vec<&str> = Vec::new();
        for theme in THEMES {
            assert!(!seen.contains(&theme.id), "`{}` is registered twice", theme.id);
            assert!(
                theme.id.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "`{}` is not a plain identifier",
                theme.id
            );
            assert!(matches!(theme.scheme, "dark" | "light"));
            seen.push(theme.id);
        }
    }

    #[test]
    fn the_stylesheet_defines_a_block_for_every_scheme() {
        let css = stylesheet();
        for theme in THEMES {
            assert!(css.contains(&format!("[data-theme=\"{}\"]", theme.id)), "{}", theme.id);
        }
        assert!(css.contains("pre .keyword"), "the code classes read the palette");
    }
}
