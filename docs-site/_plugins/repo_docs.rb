# frozen_string_literal: true

# Publishes the repository's Markdown docs as site pages, read in place.
#
# Same contract as src/guide.rs: the repository's .md files stay the only
# source, so the site and the tree never say two different things.
# - The first `# ` heading becomes the page title (rendered by the layout).
# - A link to another published doc becomes that page's site URL.
# - A link to anything else in the repository goes to the file on GitHub.
# - Unresolvable relative links are left alone and reported after the build.

module RepoDocs
  Doc = Struct.new(:src, :url, :title, :label, :section, :page, keyword_init: true)

  FENCE = /\A\s*(```|~~~)/
  H1 = /\A# (.+?)\s*#*\s*\z/

  module_function

  def url_for(src, overrides)
    return overrides[src] if overrides[src]
    return "/#{File.dirname(src)}/" if File.basename(src) == "README.md"
    "/#{src.sub(/\.md\z/, ".html")}"
  end

  # [title, body]: the first H1 outside a code fence, and the Markdown without it.
  def split_title(md, src)
    in_fence = false
    lines = md.lines
    lines.each_with_index do |line, i|
      in_fence = !in_fence if line.match?(FENCE)
      next if in_fence
      if (m = line.chomp.match(H1))
        lines.delete_at(i)
        return [m[1], lines.join]
      end
    end
    [File.basename(src, ".md"), md]
  end

  def short_label(title)
    title.delete("`").split(" — ").first.split(": ").first.strip
  end

  def escape(s)
    s.gsub("&", "&amp;").gsub("<", "&lt;").gsub(">", "&gt;").gsub('"', "&quot;")
  end

  def title_html(title)
    escape(title).gsub(/`([^`]+)`/) { "<code>#{$1}</code>" }
  end

  # Normalize a repository-relative path; nil if it escapes the repository.
  def normalize(path)
    out = []
    path.split("/").each do |part|
      case part
      when "", "." then next
      when ".." then return nil if out.empty?; out.pop
      else out << part
      end
    end
    out.join("/")
  end

  # Position of each doc in the reading order the index pages define.
  def link_order(root, files)
    order = {}
    files.each do |f|
      text = File.read(File.join(root, f))
      text.scan(/\]\(([^)\s#]+)/) do |(target)|
        path = normalize(File.join(File.dirname(f), target))
        next unless path
        path = File.join(path, "README.md") if File.directory?(File.join(root, path))
        order[path] ||= order.size
      end
    end
    order
  end

  def page_for(site, doc, body, meta)
    if doc.url.end_with?("/")
      dir, name = doc.url, "index.md"
    else
      dir, name = File.dirname(doc.url), File.basename(doc.url, ".html") + ".md"
    end
    page = Jekyll::PageWithoutAFile.new(site, site.source, dir, name)
    page.content = body
    page.data.merge!(
      "layout" => "doc",
      "title" => doc.title.delete("`"),
      "title_html" => title_html(doc.title),
      "section" => doc.section,
      "render_with_liquid" => false, # code samples contain {{ }} and {% %}
      "repo_doc" => true,
      "source_path" => doc.src,
      "source_url" => "#{meta[:github]}/blob/#{meta[:branch]}/#{doc.src}",
      "edit_url" => "#{meta[:github]}/edit/#{meta[:branch]}/#{doc.src}"
    )
    page
  end

  # Rewrite one href found in a converted doc.
  def rewrite(href, src, rt)
    return href if href.match?(%r{\A([a-z][a-z0-9+.-]*:|#|/)}i)
    path, frag = href.split("#", 2)
    return href if path.nil? || path.empty?
    anchor = frag ? "##{frag}" : ""
    resolved = normalize(File.join(File.dirname(src), path))
    unless resolved
      rt[:broken] << "#{src} -> #{href}"
      return href
    end
    candidates = [resolved, File.join(resolved, "README.md")]
    if (hit = candidates.find { |c| rt[:urls][c] })
      return rt[:baseurl] + rt[:urls][hit] + anchor
    end
    full = File.join(rt[:root], resolved)
    return "#{rt[:github]}/tree/#{rt[:branch]}/#{resolved}" if File.directory?(full)
    return "#{rt[:github]}/blob/#{rt[:branch]}/#{resolved}#{anchor}" if File.file?(full)
    rt[:broken] << "#{src} -> #{href}"
    href
  end

  def transform(html, src, rt)
    # Only real links: href text inside code samples is escaped markup, not an <a> element.
    html = html.gsub(/(<a\b[^>]*?\bhref=")([^"]+)(")/) { "#{$1}#{rewrite($2, src, rt)}#{$3}" }
    # Linkable headings.
    html = html.gsub(%r{<h([234]) id="([^"]+)">(.*?)</h\1>}m) do
      level, id, inner = Regexp.last_match(1), Regexp.last_match(2), Regexp.last_match(3)
      %(<h#{level} id="#{id}"><a class="heading-link" href="##{id}">#{inner}</a></h#{level}>)
    end
    # Wide tables scroll inside their own box.
    html.gsub("<table>", %(<div class="table-wrap"><table>)).gsub("</table>", "</table></div>")
  end

# Fail the build when converted HTML still holds Markdown block syntax or a
# script outside code: that means the renderer misread the source.
def check_converted!(html, src)
  prose = html.gsub(%r{<pre\b.*?</pre>|<code\b.*?</code>}m, "")
  raw = prose.lines.grep(/\A\s*(\#{1,6} \S|```|~~~)/)
  problems = []
  problems << "#{raw.size} line(s) of unconverted Markdown, e.g. #{raw.first.strip[0, 60].inspect}" unless raw.empty?
  problems << "a <script> element" if prose.match?(/<script\b/i)
  return if problems.empty?
  raise Jekyll::Errors::FatalException, "#{src}: #{problems.join('; ')}"
end

  # "On this page": h2/h3 of the article, inserted where the layout asks.
  def toc(output)
    article = output[%r{<article class="prose">(.*?)</article>}m, 1] || ""
    items = article.scan(%r{<h([23]) id="([^"]+)">(.*?)</h\1>}m).map do |level, id, inner|
      text = inner.gsub(/<[^>]+>/, "").strip
      %(<li data-depth="#{level}"><a href="##{id}">#{text}</a></li>)
    end
    return "" if items.size < 2
    %(<p class="toc-title">On this page</p><ul>#{items.join}</ul>)
  end

  class Generator < Jekyll::Generator
    safe true
    priority :high

    def generate(site)
      cfg = site.config.fetch("repo_docs")
      root = File.expand_path(cfg.fetch("root"), site.source)
      meta = { github: cfg.fetch("github"), branch: cfg.fetch("branch", "main") }
      overrides = cfg.fetch("urls", {})
      labels = cfg.fetch("labels", {})
      order = RepoDocs.link_order(root, cfg.fetch("order_from", []))

      docs = []
      nav = cfg.fetch("sections").map do |sec|
        files = sec.fetch("files").flat_map { |g| Dir.glob(g, base: root).sort }.uniq
        files -= docs.map(&:src) # first section to claim a file keeps it
        lead = sec.fetch("lead", [])
        files.sort_by! do |f|
          rank = f == sec["index"] ? 0 : (lead.include?(f) ? 1 : 2)
          [rank, lead.index(f) || 0, order.fetch(f, Float::INFINITY), f]
        end
        items = files.map do |f|
          title, body = RepoDocs.split_title(File.read(File.join(root, f)), f)
          doc = Doc.new(src: f, url: RepoDocs.url_for(f, overrides), title: title,
                        label: labels[f] || RepoDocs.short_label(title), section: sec.fetch("title"))
          doc.page = RepoDocs.page_for(site, doc, body, meta)
          docs << doc
          { "title" => doc.label, "url" => doc.url }
        end
        { "title" => sec.fetch("title"), "items" => items }
      end

      docs.each_with_index do |doc, i|
        prev_doc = i.positive? ? docs[i - 1] : nil
        next_doc = docs[i + 1]
        doc.page.data["prev"] = prev_doc && { "title" => prev_doc.label, "url" => prev_doc.url, "section" => prev_doc.section }
        doc.page.data["next"] = next_doc && { "title" => next_doc.label, "url" => next_doc.url, "section" => next_doc.section }
      end

      site.data["docs_nav"] = nav
      RepoDocs.runtime[site.object_id] = {
        root: root, github: meta[:github], branch: meta[:branch], baseurl: site.baseurl.to_s,
        urls: docs.to_h { |d| [d.src, d.url] }, broken: []
      }
      site.pages.concat(docs.map(&:page))
      cfg.fetch("site_assets", []).each do |rel|
        site.static_files << RootAsset.new(site, root, File.dirname(rel), File.basename(rel))
      end
      Jekyll.logger.info "Repo docs:", "#{docs.size} pages from #{root}"
    end
  end

  # A repository file served at the site root, for the local preview only (site_assets in _config.yml).
  class RootAsset < Jekyll::StaticFile
    def url = "/#{@name}"
  end

  def runtime
    @runtime ||= {}
  end
end

Jekyll::Hooks.register :pages, :post_convert do |page|
  next unless page.data["repo_doc"]
  rt = RepoDocs.runtime.fetch(page.site.object_id)
  begin
    RepoDocs.check_converted!(page.content, page.data["source_path"])
  rescue Jekyll::Errors::FatalException => e
    # Publication (ROUNDHOUSE_DOCS_LENIENT=1) never stops on one page's content: the page
    # points to its source instead of showing a misread, and the run carries a warning.
    raise unless ENV["ROUNDHOUSE_DOCS_LENIENT"] == "1"
    src = page.data["source_path"]
    puts "::warning file=#{src}::#{e.message.tr("\n", " ")}"
    page.content = %(<p>This page didn’t render cleanly in this build. <a href="#{RepoDocs.escape("#{rt[:github]}/blob/#{rt[:branch]}/#{src}")}">Read it on GitHub</a>.</p>)
    next
  end
  page.content = RepoDocs.transform(page.content, page.data["source_path"], rt)
end

Jekyll::Hooks.register :pages, :post_render do |page|
  next unless page.data["repo_doc"]
  page.output = page.output.sub("<!--repo-docs:toc-->") { RepoDocs.toc(page.output) }
end

Jekyll::Hooks.register :site, :post_write do |site|
  rt = RepoDocs.runtime[site.object_id] or next
  broken = rt[:broken].uniq
  Jekyll.logger.info "Repo docs:", "#{broken.size} unresolved relative links"
  broken.first(20).each { |b| Jekyll.logger.warn "Repo docs:", b }
  rt[:broken].clear
end
