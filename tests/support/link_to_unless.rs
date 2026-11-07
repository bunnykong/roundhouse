//! #1857: `link_to_unless`, and the block form of it and of
//! `link_to_if`. `link_to_unless cond, …` is `link_to_if !cond, …`; with
//! a block, the block is called with the name, and what it renders is
//! shown, when no link is made. Each line of `VIEW` is tagged `L<n>:`,
//! and `RENDERS` is what Rails 8.1.4 renders after the tag. The view is
//! the html-safe owner probe's app, whose `Article#label` returns
//! `"<b>a</b>".html_safe` and `Comment#label` `"<c>"`; `t` is true and
//! `f` false, both at runtime.

pub const VIEW: &str = concat!(
    "<% t = !@article.nil? %><% f = @article.nil? %>\n",
    "L0:<%= link_to_unless f, \"<c>\", \"/x\" %>\n",
    "L1:<%= link_to_unless t, \"<c>\", \"/x\" %>\n",
    "L2:<%= link_to_unless f, \"<c>\", \"/x\", class: \"k\" %>\n",
    "L3:<%= link_to_unless t, @article.label, \"/x\" %>\n",
    "L4:<%= link_to_unless f, @article.label, \"/x\" %>\n",
    "L5:<%= link_to_unless t, \"<c>\", \"/x\" do |name| %>[<%= name %>]<% end %>\n",
    "L6:<%= link_to_unless f, \"<c>\", \"/x\" do |name| %>[<%= name %>]<% end %>\n",
    "L7:<%= link_to_if f, \"<c>\", \"/x\" do |name| %>[<%= name %>]<% end %>\n",
    "L8:<%= link_to_if t, \"<c>\", \"/x\" do |name| %>[<%= name %>]<% end %>\n",
    "L9:<%= link_to_unless t, \"<c>\", \"/x\" do %>none<% end %>\n",
    "L10:<%= link_to_unless @article.nil?, \"<c>\", \"/x\" %>\n",
    "L11:<%= link_to_unless t, @article.label + @comment.label, \"/x\" %>\n",
    "L12:<%= link_to_unless t, Tracker.label, \"/x\" do %>none<% end %>|<%= Tracker.take %>\n",
);

/// `lib/tracker.rb`: `Tracker.label` logs an `n` and returns `"a"`;
/// `Tracker.take` returns the log and clears it. L12 checks that a block
/// without a parameter still evaluates the name, as Rails does.
pub const TRACKER: &str = "module Tracker\n  LOG = []\n\n  def self.label\n    LOG << \"n\"\n    \"a\"\n  end\n\n  def self.take\n    out = LOG.join\n    LOG.clear\n    out\n  end\nend\n";

pub const RENDERS: &[&str] = &[
    "L0:<a href=\"/x\">&lt;c&gt;</a>",
    "L1:&lt;c&gt;",
    "L2:<a class=\"k\" href=\"/x\">&lt;c&gt;</a>",
    "L3:<b>a</b>",
    "L4:<a href=\"/x\"><b>a</b></a>",
    "L5:[&lt;c&gt;]",
    "L6:<a href=\"/x\">&lt;c&gt;</a>",
    "L7:[&lt;c&gt;]",
    "L8:<a href=\"/x\">&lt;c&gt;</a>",
    "L9:none",
    "L10:<a href=\"/x\">&lt;c&gt;</a>",
    "L11:<b>a</b>&lt;c&gt;",
    "L12:none|n",
];
