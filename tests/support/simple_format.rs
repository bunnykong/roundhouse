//! #1861: `simple_format` with `sanitize: false`. Each line of `VIEW` is
//! tagged `L<n>:`, and `RENDERS` is what Rails 8.1.4 renders after the
//! tag. It reads no record, so every target's probe app renders it: the
//! ruby target and Spinel check each line's whole output, the strict
//! targets that the page holds each one (`content_tag_options::
//! app_rendering`). L15 to L17 are multibyte texts, which a target
//! indexing bytes would split. L5's attribute holds no `"`, which Go's
//! `html_escape` writes as `&#34;` (`html.EscapeString`, by design) where
//! Rails writes `&quot;`.

pub const VIEW: &str = concat!(
    "L0:<%= simple_format(\"a\\nb\\n\\nc\", {}, sanitize: false) %>\n",
    "L1:<%= simple_format(\"<b>a</b>\\nb\\n\\n\\nc\", {}, sanitize: false) %>\n",
    "L2:<%= simple_format(\"a\\r\\nb\\r\\n\\r\\nc\", {}, sanitize: false) %>\n",
    "L3:<%= simple_format(\"\", {}, sanitize: false) %>\n",
    "L4:<%= simple_format(nil, {}, sanitize: false) %>\n",
    "L5:<%= simple_format(\"a\\n\\nb\", { class: \"k\", id: \"x<&\" }, sanitize: false) %>\n",
    "L6:<%= simple_format(\"a\\n\\nb\", {}, wrapper_tag: :div, sanitize: false) %>\n",
    "L7:<%= simple_format(\"a\", { class: \"k\" }, wrapper_tag: \"span\", sanitize: false) %>\n",
    "L8:<%= simple_format(\"\\n\\na\", {}, sanitize: false) %>\n",
    "L9:<%= simple_format(\"a\\n\", {}, sanitize: false) %>\n",
    "L10:<%= simple_format(\"\\u3000\", {}, sanitize: false) %>\n",
    "L11:<% s = \"<b>a</b>\" + \"\\nz\" %><%= simple_format(s, {}, sanitize: false) %>\n",
    "L12:<% o = { class: \"q\" } %><%= simple_format(\"a\", o, sanitize: false) %>\n",
    "L13:<%= simple_format(\"<i>x</i>\", {}, sanitize: nil) %>\n",
    "L14:<%= simple_format(\"<i>a</i>\", {}, sanitize: false) + \"<c>\" %>\n",
    "L15:<%= simple_format(\"\\u3042\", {}, sanitize: false) %>\n",
    "L16:<%= simple_format(\"\u{e9}\\n\u{fc}\", {}, sanitize: false) %>\n",
    "L17:<%= simple_format(\"\\u3042\\n\\u3000\\u3042\", {}, sanitize: false) %>\n",
);

pub const RENDERS: &[&str] = &[
    "L0:<p>a\n<br />b</p>\n\n<p>c</p>",
    "L1:<p><b>a</b>\n<br />b</p>\n\n<p>c</p>",
    "L2:<p>a\n<br />b</p>\n\n<p>c</p>",
    "L3:<p></p>",
    "L4:<p></p>",
    "L5:<p class=\"k\" id=\"x&lt;&amp;\">a</p>\n\n<p class=\"k\" id=\"x&lt;&amp;\">b</p>",
    "L6:<div>a</div>\n\n<div>b</div>",
    "L7:<span class=\"k\">a</span>",
    "L8:<p></p>\n\n<p>a</p>",
    "L9:<p>a\n</p>",
    "L10:<p></p>",
    "L11:<p><b>a</b>\n<br />z</p>",
    "L12:<p class=\"q\">a</p>",
    "L13:<p><i>x</i></p>",
    "L14:<p><i>a</i></p>&lt;c&gt;",
    "L15:<p>\u{3042}</p>",
    "L16:<p>\u{e9}\n<br />\u{fc}</p>",
    "L17:<p>\u{3042}\n<br />\u{3000}\u{3042}</p>",
];
