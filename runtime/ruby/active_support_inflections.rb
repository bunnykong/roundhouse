# frozen_string_literal: true
# Pure string ActiveSupport helpers for the Ruby/Spinel scaffold.
# `active_support_ext.rb` keeps blank?/present?/… (`is_a?` dispatch
# that AOT targets cannot host) and requires this file so scaffold
# trees share one ActiveSupport module. Not yet in strict-target
# runtime_loader tables — their string emit does not host the
# char-walk bodies; controller_name/path are literalized instead.
module ActiveSupport
  # Not ActiveSupport's regex pipeline: without its acronym and human
  # tables the steps it runs are these string walks.
  def self.underscore(text)
    s = text.to_s.split("::").join("/")
    out = +""
    n = s.length
    i = 0
    while i < n
      c = s[i].to_s
      if c >= "A" && c <= "Z" && i > 0
        prev = s[i - 1].to_s
        nxt = i + 1 < n ? s[i + 1].to_s : ""
        prev_lower = (prev >= "a" && prev <= "z") || (prev >= "0" && prev <= "9")
        prev_upper = prev >= "A" && prev <= "Z"
        next_lower = nxt >= "a" && nxt <= "z"
        out << "_" if prev_lower || (prev_upper && next_lower)
      end
      out << (c == "-" ? "_" : c)
      i = i + 1
    end
    out.downcase
  end

  # `String#remove(*patterns)` — literal substring delete, not a Regexp
  # rewrite. Patterns that are not strings are stringified once.
  def self.remove(text, pattern)
    text.to_s.split(pattern.to_s).join("")
  end

  # `String#strip_heredoc`: Rails removes `scan(/^[ \t]*(?=\S)/).min` from each line's start,
  # the least indentation by String order, not by length, of the lines that are not blank.
  def self.strip_heredoc(text)
    lines = text.split("\n", -1)
    least = ""
    found = false
    lines.each do |line|
      i = 0
      while i < line.length && (line[i] == " " || line[i] == "\t")
        i = i + 1
      end
      next if i >= line.length
      c = line[i]
      next if c == "\r" || c == "\f" || c == "\v"
      lead = line[0, i].to_s
      if !found || lead < least
        least = lead
        found = true
      end
    end
    out = least.empty? ? text.dup : lines.map { |line| line.start_with?(least) ? line[least.length, line.length - least.length].to_s : line }.join("\n")
    out.freeze if text.frozen?
    out
  end

  def self.demodulize(text)
    s = text.to_s
    idx = s.rindex("::")
    # Not `s[(idx + 2)..-1]` or `if idx`: a strict target slices by start and length, and does not narrow a found index once tested
    return s if idx.nil?
    at = idx.to_i
    s[at + 2, s.length - at - 2].to_s
  end

  # Regular-suffix singularize only. Analyze folds `String#singularize`
  # into an ivar name only when this answer matches `naming::singularize`
  # (irregular / uncountable tables); otherwise the ivar stays unresolved.
  def self.singularize(text)
    s = text.to_s
    return s if s.empty?
    return s[0, s.length - 3].to_s + "y" if s.end_with?("ies") && s.length > 3
    return s[0, s.length - 2].to_s if s.end_with?("ses") || s.end_with?("xes") || s.end_with?("zes") || s.end_with?("ches") || s.end_with?("shes")
    return s[0, s.length - 1].to_s if s.end_with?("s") && !s.end_with?("ss")
    s
  end

  def self.humanize(text)
    raise NoMethodError, "undefined method 'humanize' for nil" if text.nil?
    s = text.to_s.tr("_", " ").lstrip
    s = s[0, s.length - 3].to_s if s.end_with?(" id")
    s = s.downcase
    return s if s.empty?
    s[0].to_s.upcase + s[1, s.length - 1].to_s
  end

  def self.titleize(text)
    raise NoMethodError, "undefined method 'titleize' for nil" if text.nil?
    s = humanize(underscore(text))
    out = +""
    n = s.length
    i = 0
    while i < n
      c = s[i].to_s
      prev = i > 0 ? s[i - 1].to_s : ""
      word_prev = (prev >= "a" && prev <= "z") || (prev >= "A" && prev <= "Z") || (prev >= "0" && prev <= "9") || prev == "_"
      out << (c >= "a" && c <= "z" && !word_prev ? c.upcase : c)
      i = i + 1
    end
    out
  end

  # The String methods over the Inflector bodies below. A nil receiver
  # raises, as Rails' String methods do, naming the method the caller
  # wrote (an alias included); otherwise the Inflector body answers.
  def self.camelize(text)
    raise NoMethodError, "undefined method 'camelize' for nil" if text.nil?
    ActiveSupport::Inflector.camelize(text.to_s)
  end

  def self.camelcase(text)
    raise NoMethodError, "undefined method 'camelcase' for nil" if text.nil?
    ActiveSupport::Inflector.camelize(text.to_s)
  end

  def self.titlecase(text)
    raise NoMethodError, "undefined method 'titlecase' for nil" if text.nil?
    titleize(text)
  end

  def self.classify(text)
    raise NoMethodError, "undefined method 'classify' for nil" if text.nil?
    ActiveSupport::Inflector.classify(text.to_s)
  end

  def self.dasherize(text)
    raise NoMethodError, "undefined method 'dasherize' for nil" if text.nil?
    ActiveSupport::Inflector.dasherize(text.to_s)
  end

  def self.deconstantize(text)
    raise NoMethodError, "undefined method 'deconstantize' for nil" if text.nil?
    ActiveSupport::Inflector.deconstantize(text.to_s)
  end

  def self.foreign_key(text)
    raise NoMethodError, "undefined method 'foreign_key' for nil" if text.nil?
    ActiveSupport::Inflector.foreign_key(text.to_s)
  end
end

# Rails 8.1.4 Inflector methods that do not depend on pluralization,
# transliteration, constant lookup, or locale-specific rule catalogs. String
# methods match default ASCII identifier behavior; configured acronym tables
# are intentionally not modeled. `classify` is the one exception: it takes
# its singular from the regular-suffix `ActiveSupport.singularize` above, the
# rules `String#singularize` already grounds to. `tableize`, locale mutation,
# constantization, transliteration, and parameterization remain unexposed.
module ActiveSupport
  module Inflector
    # Rails' `Inflector.pluralize(word)` and `singularize(word)` under the default rules (view_component names a
    # slot's methods with them); the String forms' own ports answer.
    def self.pluralize(word)
      ::Inflector.pluralize_word(word.to_s, 2)
    end

    # Rails 8.1's default singular rules, latest defined first, after its uncountables and irregulars (the
    # regular-suffix `ActiveSupport.singularize` the String form uses would give "movy" and "mice").
    def self.singularize(word)
      s = word.to_s
      d = s.downcase
      n = s.length
      return s if s.empty? || ::Inflector.pluralize_uncountable?(d)
      return ::Inflector.pluralize_cased(s, 6, "erson") if d.end_with?("people") || d.end_with?("person")
      return ::Inflector.pluralize_cased(s, 8, "hild") if d.end_with?("children")
      return ::Inflector.pluralize_cased(s, 5, "hild") if d.end_with?("child")
      return ::Inflector.pluralize_cased(s, 3, "an") if d.end_with?("men") || d.end_with?("man")
      return ::Inflector.pluralize_cased(s, 5, "ex") if d.end_with?("sexes")
      return ::Inflector.pluralize_cased(s, 5, "ove") if d.end_with?("moves")
      return ::Inflector.pluralize_cased(s, 7, "ombie") if d.end_with?("zombies")
      return singular_tail(s, 1, "") if d.end_with?("databases")
      return singular_tail(s, 3, "") if d.end_with?("quizzes")
      return singular_tail(s, 4, "ix") if d.end_with?("matrices")
      return singular_tail(s, 4, "ex") if d.end_with?("vertices") || d.end_with?("indices")
      return s[0, 2].to_s + s[4, n - 4].to_s if d.start_with?("oxen")
      return singular_tail(s, 2, "") if d.end_with?("aliases") || d.end_with?("statuses")
      return s if d.end_with?("alias") || d.end_with?("status")
      return singular_tail(s, 1, "us") if d.end_with?("octopi") || d.end_with?("viri")
      return s if d.end_with?("octopus") || d.end_with?("virus")
      return s[0, 1].to_s + "xis" if d == "axes" || d == "axis"
      return singular_tail(s, 2, "is") if d.end_with?("crises") || d.end_with?("crisis") || d.end_with?("testes") || d.end_with?("testis")
      return singular_tail(s, 1, "") if d.end_with?("shoes")
      return singular_tail(s, 2, "") if d.end_with?("oes")
      return singular_tail(s, 2, "") if d.end_with?("buses")
      return s if d.end_with?("bus")
      return s[0, 1].to_s + "ouse" if d == "mice" || d == "lice"
      return singular_tail(s, 2, "") if d.end_with?("xes") || d.end_with?("ches") || d.end_with?("sses") || d.end_with?("shes")
      return singular_tail(s, 1, "") if d.end_with?("movies")
      return s if d.end_with?("series")
      return singular_tail(s, 3, "y") if d.end_with?("ies") && n >= 4 && (!"aeiouy".include?(d[n - 4].to_s) || (n >= 5 && d[n - 5, 2] == "qu"))
      return singular_tail(s, 3, "f") if d.end_with?("lves") || d.end_with?("rves")
      return singular_tail(s, 1, "") if d.end_with?("tives") || d.end_with?("hives")
      return singular_tail(s, 3, "fe") if d.end_with?("ves") && n >= 4 && d[n - 4] != "f"
      return s[0, 5].to_s + "sis" if d == "analyses" || d == "analysis"
      if d.end_with?("ses") || d.end_with?("sis")
        stem = d[0, n - 3].to_s
        if stem.end_with?("analy") || stem.end_with?("ba") || stem.end_with?("diagno") || stem.end_with?("parenthe") ||
            stem.end_with?("progno") || stem.end_with?("synop") || stem.end_with?("the")
          return singular_tail(s, 3, "sis")
        end
      end
      return singular_tail(s, 1, "um") if d.end_with?("ta") || d.end_with?("ia")
      return s if d.end_with?("news") || d.end_with?("ss")
      return singular_tail(s, 1, "") if d.end_with?("s")
      s
    end

    def self.singular_tail(s, drop, rest)
      s[0, s.length - drop].to_s + rest
    end

    # Rails' camelize with the default (upper camel case) and lower-camel
    # option. A slash separates namespaces; an underscore starts a new word.
    def self.camelize(text, uppercase_first_letter = true)
      s = text.to_s
      out = +""
      n = s.length
      i = 0
      capitalize = uppercase_first_letter
      normalize_word = false

      if s.start_with?("::")
        out << "::"
        i = 2
        capitalize = false
      elsif s.start_with?("/")
        out << "::"
        i = 1
        capitalize = true
        normalize_word = true
      end

      while i < n
        c = s[i].to_s
        if c == "/"
          out << "::"
          capitalize = true
          normalize_word = true
        elsif c == "_"
          capitalize = true
          normalize_word = true
        else
          ascii_word_character = (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") || (c >= "0" && c <= "9")
          normalize_word = false if normalize_word && !ascii_word_character
          if i == 0 && !uppercase_first_letter
            out << c.downcase
          else
            out << (capitalize ? c.upcase : (normalize_word ? c.downcase : c))
          end
          capitalize = false
        end
        i = i + 1
      end
      out
    end

    def self.underscore(text)
      ActiveSupport.underscore(text)
    end

    # Rails' dasherize: every underscore becomes a dash.
    def self.dasherize(text)
      text.to_s.tr("_", "-")
    end

    # Rails' classify: drop a leading schema name ("shop.gear_boxes"),
    # singularize, camelize. An irregular plural ("people") keeps the
    # answer `ActiveSupport.singularize` gives it.
    def self.classify(table_name)
      s = table_name.to_s
      idx = s.rindex(".")
      # Not `s[(idx + 1)..-1]` or `idx ? … : s`: a strict target slices by start and length, and does not narrow a found index once tested
      base = idx.nil? ? s : s[idx.to_i + 1, s.length - idx.to_i - 1].to_s
      camelize(ActiveSupport.singularize(base))
    end

    # Rails' String-based namespace removal; this does not resolve constants.
    def self.deconstantize(text)
      s = text.to_s
      idx = s.rindex("::")
      # Not `idx ? s[0, idx] : ""`: a strict target does not narrow a found index once tested
      idx.nil? ? "" : s[0, idx.to_i].to_s
    end

    # Rails assumes a top-level class name and removes its namespace before
    # underscoring. The optional separator flag is significant to callers.
    def self.foreign_key(class_name, separate_class_name_and_id_with_underscore = true)
      base = ActiveSupport.underscore(ActiveSupport.demodulize(class_name))
      base + (separate_class_name_and_id_with_underscore ? "_id" : "id")
    end

    # Uppercase only the first character; preserve the remainder unchanged.
    def self.upcase_first(text)
      s = text.to_s
      return s if s.empty?
      s[0].to_s.upcase + s[1, s.length - 1].to_s
    end

    # Lowercase only the first character; preserve the remainder unchanged.
    def self.downcase_first(text)
      s = text.to_s
      return s if s.empty?
      s[0].to_s.downcase + s[1, s.length - 1].to_s
    end

    # Rails' default integer ordinal suffix rules. Configuration and
    # locale-specific ActiveSupport behavior do not affect this slice.
    def self.ordinalize(number)
      magnitude = number.abs
      remainder_hundred = magnitude % 100
      suffix = "th"
      if remainder_hundred < 11 || remainder_hundred > 13
        remainder_ten = magnitude % 10
        if remainder_ten == 1
          suffix = "st"
        elsif remainder_ten == 2
          suffix = "nd"
        elsif remainder_ten == 3
          suffix = "rd"
        end
      end
      number.to_s + suffix
    end
  end
end
