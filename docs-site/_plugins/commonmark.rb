# frozen_string_literal: true

# Markdown for the docs pages: CommonMark with GitHub's extensions (commonmarker,
# i.e. comrak), the dialect GitHub and src/guide.rs (pulldown-cmark) render these
# files in. Selected by `markdown: RoundhouseCommonmark` in _config.yml.
#
# Code blocks are highlighted with Rouge, in the markup Jekyll's own highlighter
# produces, so docs/assets/rouge.css applies unchanged.

require "cgi"
require "set"
require "commonmarker"
require "rouge"
require "sanitize"

module Jekyll
  module Converters
    class Markdown
      class RoundhouseCommonmark
        OPTIONS = {
          render: { unsafe: true, github_pre_lang: true, hardbreaks: false },
          extension: { table: true, strikethrough: true, autolink: true, tasklist: true, header_ids: "" }
        }.freeze

        def initialize(config)
          @config = config
        end

        # Raw HTML a doc may use on purpose. Any other tag in prose is shown as
        # text, like GitHub's tag filter but stricter: an unclosed <textarea> or
        # <script> from a misparsed code span would otherwise swallow the page.
        PROSE_TAGS = %w[
          a abbr b blockquote br code dd del details div dl dt em h1 h2 h3 h4 h5 h6 hr i img input
          kbd li mark ol p pre q s samp section small span strong sub summary sup table tbody td
          tfoot th thead tr u ul var
        ].to_set.freeze

        # The security boundary for raw HTML: an HTML5 parser keeps these elements,
        # attributes and URL schemes and drops the rest (event handlers, style,
        # javascript: in any encoding), as GitHub's filter does.
        SANITIZE = Sanitize::Config.freeze_config(
          elements: PROSE_TAGS.to_a,
          attributes: {
            all: %w[id title lang dir],
            "a" => %w[href name aria-label data-footnote-ref data-footnote-backref data-footnote-backref-idx],
            "img" => %w[src alt width height align],
            "input" => %w[type checked disabled],
            "ol" => %w[start type reversed],
            "td" => %w[align colspan rowspan], "th" => %w[align colspan rowspan],
            "details" => %w[open], "section" => %w[data-footnotes], "blockquote" => %w[cite], "q" => %w[cite],
            "p" => %w[align], "div" => %w[align]
          },
          protocols: {
            "a" => { "href" => ["http", "https", "mailto", :relative] },
            "img" => { "src" => ["http", "https", :relative] },
            "blockquote" => { "cite" => ["http", "https", :relative] }, "q" => { "cite" => ["http", "https", :relative] }
          }
        )

        def convert(content)
          html = Commonmarker.to_html(content, options: OPTIONS, plugins: { syntax_highlighter: nil })
          html = html.gsub(%r{<a href="#[^"]*"[^>]*class="anchor"[^>]*></a>}, "")
          html = Sanitize.fragment(escape_unknown_tags(html), SANITIZE)
          # Column alignment as pulldown-cmark writes it (`align` is obsolete HTML),
          # and an empty header cell as a plain cell: it names no column.
          html = html.gsub(/<(t[dh]) align="(left|center|right)">/, '<\1 style="text-align: \2">')
                     .gsub(%r{<th( style="[^"]*")?></th>}, '<td\1></td>')
          highlight(html)
        end

        private

        # Display, not security: a tag outside PROSE_TAGS shows as text. Code is
        # already escaped, so only raw HTML is affected, including raw HTML in a
        # <pre>.
        def escape_unknown_tags(html)
          html.gsub(%r{</?([a-zA-Z][\w-]*)\b[^>]*>}) do |tag|
            PROSE_TAGS.include?(Regexp.last_match(1).downcase) ? tag : CGI.escapeHTML(tag)
          end
        end

        def highlight(html)
          html = html.gsub(%r{<pre lang="([^"]+)"><code>(.*?)</code></pre>}m) do
            lang = Regexp.last_match(1)
            code = CGI.unescapeHTML(Regexp.last_match(2))
            lexer = Rouge::Lexer.find_fancy(lang, code) || Rouge::Lexers::PlainText.new
            block(Rouge::Formatters::HTML.new.format(lexer.lex(code)), lang)
          end
          html.gsub(%r{<pre><code>(.*?)</code></pre>}m) { block(Regexp.last_match(1), nil) }
        end

        def block(body, lang)
          klass = lang ? "language-#{CGI.escapeHTML(lang)} highlighter-rouge" : "highlighter-rouge"
          # tabindex: code wider than the page scrolls from the keyboard too, as on GitHub.
          %(<div class="#{klass}"><div class="highlight"><pre class="highlight" tabindex="0"><code>#{body}</code></pre></div></div>)
        end
      end
    end
  end
end
