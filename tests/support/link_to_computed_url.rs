//! #1865: `link_to` with a url the template computes: a String sum, a
//! String local, a String method's result, a route helper with a String
//! added, and the block form over a String local. Each line of `VIEW` is
//! tagged `L<n>:`, and `RENDERS` is what Rails 8.1.4 renders after the
//! tag. It reads no record, so every target's probe app renders it: the
//! ruby target and Spinel check each line's whole output, the strict
//! targets that the page holds each one (`content_tag_options::
//! app_rendering`). L4's url holds `&` and `<`, which the href escapes.

pub const VIEW: &str = concat!(
    r#"L0:<%= link_to "b", "/x" + "y" %>"#, "\n",
    r#"L1:<%= link_to "d", "/x" + "y", class: "k" %>"#, "\n",
    r#"L2:<% u = "/x" %><%= link_to "f", u %>"#, "\n",
    r#"L3:<%= link_to "g", "/X".downcase %>"#, "\n",
    r#"L4:<%= link_to "h", "/x?a=1" + "&b=<2>" %>"#, "\n",
    r#"L5:<%= link_to "i", articles_path + "?p=1" %>"#, "\n",
    r#"L6:<% v = "/v" %><%= link_to "j", v, class: "k" %>"#, "\n",
    r#"L7:<%= link_to "<k>", "/x" + "y" %>"#, "\n",
    r#"L8:<% w = "/w" %><%= link_to w do %>blk<% end %>"#, "\n",
);

pub const RENDERS: &[&str] = &[
    r#"L0:<a href="/xy">b</a>"#,
    r#"L1:<a class="k" href="/xy">d</a>"#,
    r#"L2:<a href="/x">f</a>"#,
    r#"L3:<a href="/x">g</a>"#,
    r#"L4:<a href="/x?a=1&amp;b=&lt;2&gt;">h</a>"#,
    r#"L5:<a href="/articles?p=1">i</a>"#,
    r#"L6:<a class="k" href="/v">j</a>"#,
    r#"L7:<a href="/xy">&lt;k&gt;</a>"#,
    r#"L8:<a href="/w">blk</a>"#,
];
