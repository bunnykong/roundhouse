# frozen_string_literal: true
# CRuby-only ActiveSupport core extensions (Object / String / Array).
#
# The reopened-builtin surface the lobsters corpus actually calls —
# ActiveSupport itself ships these as core_ext monkey-patches, so the
# CRuby overlay mirrors that shape. Reopening builtins is overlay-only
# (the shared transpiled runtime can't); strict targets get typed
# lowerings for these when lobsters comes up on them.
#
# Inflection here is the rule subset the corpus words exercise
# (story→stories, category→categories, comment→comments, …), not
# ActiveSupport's full irregular/uncountable tables.

class Object
  # `blank?`/`present?` (core_ext/object/blank.rb). The generic form
  # covers nil (!nil → true), false, numerics, and containers via
  # `empty?`. lobsters calls `present?` ~113 times.
  def blank?
    respond_to?(:empty?) ? !!empty? : !self
  end

  def present?
    !blank?
  end

  # `presence` (core_ext/object/blank.rb): the receiver when present,
  # else nil — the `url.presence || fallback` idiom.
  def presence
    present? ? self : nil
  end

  # `to_param` (core_ext/object/to_param): the URL-segment form of a
  # value, `to_s` by default. Models with a custom `to_param` (lobsters'
  # Domain routes on its name) override this; generated route helpers
  # call it on every segment arg.
  def to_param
    to_s
  end

  # `deep_dup` (core_ext/object/deep_dup.rb) on a receiver inference
  # could not type; a typed Hash/Array is grounded to the same function
  # by `lower::symbolize_keys`.
  def deep_dup
    ActiveSupport.deep_dup(self)
  end
end

# `compact_blank` (core_ext/enumerable.rb + core_ext/hash.rb) — the
# blank family's collection form, and the one member of it `lower::blank`
# cannot always ground.
#
# That pass rewrites `xs.compact_blank` through the ELEMENT type and
# files a `blank_unlowered` diagnostic when the element is untyped,
# saying in as many words that "the CRuby overlay serves it at
# runtime". It did not: nothing here defined the method, so the ledger
# entry promised a fallback that raised NoMethodError. campfire's
# `sessions.pluck(:ip_address).compact_blank` (a concern body, where
# `self` is not yet typed) is the site that showed it.
#
# Enumerable, not Array: Rails defines it there, so a Hash keeps its
# own pair-shaped override below and every other collection gets the
# element form.
module Enumerable
  def compact_blank
    reject(&:blank?)
  end
end

class Hash
  # Rails rejects on the VALUE and keeps the key — `{ host: nil,
  # protocol: "https" }.compact_blank` is `{ protocol: "https" }`,
  # which is how campfire's `SetCurrentRequest` builds default_url_options.
  def compact_blank
    reject { |_k, v| v.blank? }
  end
end

# A string an app has explicitly marked html-safe (`raw(...)`,
# `.html_safe`) — the overlay's ActiveSupport::SafeBuffer equivalent.
# The only behavior it adds is `html_safe?` returning true, which the
# overlay's escape-aware `html_escape` honors (see
# action_view_safe_buffer.rb). Escaping is still decided positionally
# at emit time wherever the walker can see the producer; SafeString
# carries safety across the value boundaries it can't — a `raw()`
# label threaded through an app helper's parameter into `link_to`, or
# a model method that ends in `.html_safe` (lobsters' Hat#to_html_label).
class SafeString < String
  def initialize(value)
    super(value)
    @html_safe = true
  end

  def html_safe?
    @html_safe
  end

  def html_safe
    @html_safe = true
    self
  end

  # Rails' SafeBuffer marks itself unsafe when squish! invokes gsub! or
  # strip!, even when neither call changes the contents.
  def squish!
    super
    @html_safe = false
    self
  end

  # String#to_s on a subclass returns a plain String copy; safety must
  # survive the `.to_s` coercions the emit sprinkles on helper args.
  def to_s
    self
  end

  # Rails' `SafeBuffer#safe_concat`: appended as it is, the buffer still safe.
  def safe_concat(value)
    concat(value.to_s)
  end

  # Rails' `SafeBuffer#+`: still safe, the other side escaped unless it is safe too (a builder's `label + text_field`).
  def +(other)
    return String.new(self) + other.to_s unless html_safe?
    SafeString.new(String.new(self) + ActionView::ViewHelpers.html_escape(other))
  end
end

class String
  # AS specializes String#blank? to treat whitespace-only as blank.
  def blank?
    empty? || match?(/\A[[:space:]]*\z/)
  end

  # Rails' destructive form returns the receiver even when unchanged.
  def squish!
    gsub!(/[[:space:]]+/, " ")
    strip!
    self
  end

  # AS `truncate` (core_ext/string/filters.rb) for a receiver the
  # lowering could not prove a String. A typed one is grounded to
  # `ActiveSupport.truncate`; this reopen also takes Rails' Regexp
  # separator, which CRuby's `rindex` answers.
  def truncate(truncate_to, options = {})
    return dup unless length > truncate_to

    omission = options[:omission] || "..."
    room = truncate_to - omission.length
    return omission.dup if room <= 0

    stop = options[:separator] ? (rindex(options[:separator], room) || room) : room
    +"#{self[0, stop]}#{omission}"
  end

  # AS `truncate_words` (core_ext/string/filters.rb) for a receiver the
  # lowering could not prove a String. A typed one with the default
  # separator is grounded to `ActiveSupport.truncate_words`.
  def truncate_words(words_count, options = {})
    sep = options[:separator] || /\s+/
    sep = Regexp.escape(sep.to_s) unless sep.is_a?(Regexp)
    if self =~ /\A((?>.+?#{sep}){#{words_count - 1}}.+?)#{sep}.*/m
      $1 + (options[:omission] || "...")
    else
      dup
    end
  end

  # Not `options[:omission] || "…"`: AS reads an explicit `omission: nil` as no omission.
  def truncate_bytes(truncate_to, omission: "\u2026")
    ActiveSupport.truncate_bytes(self, truncate_to, omission)
  end

  # `html_safe` promotes to the marked type; plain strings answer
  # `html_safe?` false so the escape-aware `html_escape` escapes them.
  # (Both were identity/true in the earlier positional-only world —
  # nothing consumed `html_safe?` then; the SafeString-aware
  # html_escape override is its first consumer.)
  def html_safe
    SafeString.new(self)
  end

  def html_safe?
    false
  end

  # `'story'.pluralize(count)` — singular when count == 1, else the
  # plural form. Every corpus call site passes a count; the inflection
  # applies to the string's tail, matching AS ("#{n} story".pluralize(n)
  # → "2 stories"). A trailing "s" is treated as already-plural
  # (idempotent, like AS's /s$/ → "s" rule).
  def pluralize(count = nil)
    return self if count == 1
    if match?(/([^aeiouy]|qu)y\z/i)
      sub(/y\z/i, "ies")
    elsif end_with?("s")
      self
    elsif match?(/(x|z|ch|sh)\z/i)
      "#{self}es"
    else
      "#{self}s"
    end
  end

  def singularize
    if match?(/([^aeiouy]|qu)ies\z/i)
      sub(/ies\z/i, "y")
    elsif match?(/(s|x|z|ch|sh)es\z/i)
      sub(/es\z/i, "")
    elsif end_with?("s") && !end_with?("ss")
      sub(/s\z/i, "")
    else
      self
    end
  end

  # AS `parameterize`: downcase, squash non-alphanumerics to the
  # separator, collapse runs, trim edges. (No transliteration pass —
  # the corpus titles are ASCII; non-ASCII chars drop like Rails'
  # post-transliterate "?" placeholders do under gsub.)
  def parameterize(separator: "-")
    # Default-separator fast path: literal regexes are compiled once and
    # cached per call-site, where the general path below rebuilds two
    # regexes from `Regexp.escape(separator)` on every call. `separator`
    # is "-" for essentially every caller (Story#title_as_url et al.),
    # so this halves both the time and the allocations of the hottest
    # parameterize sites while staying byte-identical.
    if separator == "-"
      return downcase
        .gsub(/[^a-z0-9\-_]+/, "-")
        .gsub(/-{2,}/, "-")
        .gsub(/\A-|-\z/, "")
    end
    downcase
      .gsub(/[^a-z0-9\-_]+/, separator)
      .gsub(/#{Regexp.escape(separator)}{2,}/, separator)
      .gsub(/\A#{Regexp.escape(separator)}|#{Regexp.escape(separator)}\z/, "")
  end
end

class Array
  # AS `to_sentence` with its three connector options, :en defaults:
  # "a", "a and b", "a, b, and c".
  def to_sentence(words_connector: ", ", two_words_connector: " and ", last_word_connector: ", and ")
    case length
    when 0 then ""
    when 1 then self[0].to_s
    when 2 then "#{self[0]}#{two_words_connector}#{self[1]}"
    else "#{self[0..-2].join(words_connector)}#{last_word_connector}#{self[-1]}"
    end
  end

  # AS's ordinal readers (core_ext/array/access.rb). Rails ships
  # `second` through `fifth` plus `forty_two`; the corpus reaches
  # `third` (campfire's `messages_controller_test` pages relative to
  # `@messages.third`), and the neighbours come with it because they
  # are the same one-liner and a list that stops at the first caller is
  # the shape that drifts.
  #
  # `forty_two` is deliberately NOT here: it is a joke method, and a
  # reader who meets it in this file should find it in Rails' docs
  # rather than in ours.
  def second
    self[1]
  end

  def third
    self[2]
  end

  def fourth
    self[3]
  end

  def fifth
    self[4]
  end

  # AS `without` / `excluding` (core_ext/enumerable.rb): the receiver
  # minus the named elements. `Relation` already answers both (they are
  # what `Room.opens.without(current)` needs); an Array did not, and a
  # test body holding a materialized association is exactly where the
  # Array form turns up — campfire's `rooms/refreshes_controller_test`
  # writes `@room.messages.without(@old_message)`.
  #
  # Rails compares with `==`; two loads of the same row are different
  # objects here, so this is IDENTITY on a model unless the model
  # defines `==`. Recorded rather than worked around: `Base` documents
  # why it defines no equality protocol.
  def without(*values)
    reject { |x| values.include?(x) }
  end

  alias_method :excluding, :without
end

# Not over ActiveSupport itself where a framework test loads it beside this file: its `with_options` is the one.
unless Object.method_defined?(:with_options)
  class Object
    def with_options(options, &block)
      merger = ActiveSupport::OptionMerger.new(self, options)
      return merger unless block
      block.arity.zero? ? merger.instance_eval(&block) : block.call(merger)
    end
  end

  module ActiveSupport
    # Not a BasicObject: ActiveSupport's own is an Object, and a framework test that loads it after this file reopens it.
    class OptionMerger
      instance_methods.each do |method|
        undef_method(method) unless method.start_with?("__", "instance_eval", "class", "object_id")
      end

      def initialize(context, options)
        @context = context
        @options = options
      end

      private

      def method_missing(method, *arguments, &block)
        options = arguments.last.respond_to?(:to_hash) ? deep_merge(@options, arguments.pop.to_hash) : @options
        @context.__send__(method, *arguments, **options, &block)
      end

      # Rails' `Hash#deep_merge`, which this runtime does not reopen Hash with.
      def deep_merge(base, other)
        base.merge(other) { |_, mine, theirs| mine.is_a?(::Hash) && theirs.is_a?(::Hash) ? deep_merge(mine, theirs) : theirs }
      end

      def respond_to_missing?(*arguments)
        @context.respond_to?(*arguments)
      end
    end
  end
end

# Not for the app's own calls, which the lowering grounds: a gem's file runs this as it loads (Devise's csrf_cleaner).
# Not over ActiveSupport itself where a framework test loads it beside this file: its `try` is the one.
unless Object.method_defined?(:try)
  class Object
    def try(*args, &block)
      if args.empty? && block
        block.arity.zero? ? instance_eval(&block) : yield(self)
      elsif !args.empty? && respond_to?(args.first)
        public_send(*args, &block)
      end
    end

    def try!(*args, &block)
      if args.empty? && block
        block.arity.zero? ? instance_eval(&block) : yield(self)
      else
        public_send(*args, &block)
      end
    end
  end

  class NilClass
    def try(*)
      nil
    end

    def try!(*)
      nil
    end
  end
end

# ActiveSupport's `Class#class_attribute` (core_ext/class/attribute.rb), for a call the class body does not write
# itself, which the ingest lowers: a gem's `self.extended(base)` hook's `base.class_attribute :mappings`.
unless Class.method_defined?(:class_attribute)
  class Class
    def class_attribute(*attrs, instance_accessor: true, instance_reader: instance_accessor, instance_writer: instance_accessor, instance_predicate: true, default: nil)
      attrs.each do |name|
        ivar = :"@#{name}"
        singleton_class.send(:redefine_class_attribute, name, default)
        singleton_class.define_method(:"#{name}=") do |value|
          singleton_class.send(:redefine_class_attribute, name, value)
          value
        end
        singleton_class.define_method(:"#{name}?") { !!public_send(name) } if instance_predicate
        if instance_reader
          define_method(name) { instance_variable_defined?(ivar) ? instance_variable_get(ivar) : self.class.public_send(name) }
          define_method(:"#{name}?") { !!public_send(name) } if instance_predicate
        end
        define_method(:"#{name}=") { |value| instance_variable_set(ivar, value) } if instance_writer
      end
    end
  end

  class Module
    private

    # Not `define_method` over the old one: Ruby warns of each redefinition under -w, as Rails silences it.
    def redefine_class_attribute(name, value)
      remove_method(name) if method_defined?(name, false) || private_method_defined?(name, false)
      define_method(name) { value }
    end
  end
end

# ActiveSupport's String inflections and Hash/Array extensions, for a receiver the analysis did not type, which the
# lowering would otherwise rewrite to the typed runtime's (`ActiveSupport.camelize(s)`): a gem's own code, such as
# simple_form's `options.reverse_merge!(…)`. Each only where ActiveSupport itself has not defined it.
class String
  unless method_defined?(:camelize)
    def camelize(first_letter = :upper)
      s = sub(/^[a-z\d]*/) { |m| first_letter == :lower ? m : m.capitalize }
      s.gsub(%r{(?:_|(/))([a-z\d]*)}i) { "#{$1}#{$2.capitalize}" }.gsub("/", "::")
    end
  end

  unless method_defined?(:underscore)
    def underscore
      return dup unless match?(/[A-Z-]|::/)
      gsub("::", "/").gsub(/([A-Z\d]+)([A-Z][a-z])/, '\1_\2').gsub(/([a-z\d])([A-Z])/, '\1_\2').tr("-", "_").downcase
    end
  end

  unless method_defined?(:humanize)
    def humanize(capitalize: true, keep_id_suffix: false)
      s = tr("_", " ").lstrip
      s = s.delete_suffix(" id") unless keep_id_suffix
      s = s.downcase
      capitalize ? s.sub(/\A\w/) { |m| m.upcase } : s
    end
  end

  unless method_defined?(:demodulize)
    def demodulize
      (i = rindex("::")) ? self[(i + 2)..] : dup
    end
  end

  unless method_defined?(:constantize)
    def constantize
      Object.const_get(self)
    end
  end
end

class Hash
  unless method_defined?(:reverse_merge)
    def reverse_merge(other_hash)
      other_hash.merge(self)
    end

    def reverse_merge!(other_hash)
      replace(reverse_merge(other_hash))
    end
    alias_method :with_defaults, :reverse_merge
    alias_method :with_defaults!, :reverse_merge!
  end

  unless method_defined?(:deep_merge)
    def deep_merge(other_hash, &block)
      dup.deep_merge!(other_hash, &block)
    end

    def deep_merge!(other_hash, &block)
      merge!(other_hash) do |key, this_val, other_val|
        if this_val.is_a?(Hash) && other_val.is_a?(Hash)
          this_val.deep_merge(other_val, &block)
        elsif block
          block.call(key, this_val, other_val)
        else
          other_val
        end
      end
    end
  end

  unless method_defined?(:stringify_keys)
    def stringify_keys
      transform_keys(&:to_s)
    end

    def symbolize_keys
      transform_keys { |key| key.to_sym rescue key }
    end
  end

  unless method_defined?(:extractable_options?)
    def extractable_options?
      instance_of?(Hash)
    end
  end
end

class Array
  unless method_defined?(:extract_options!)
    def extract_options!
      last.is_a?(Hash) && last.extractable_options? ? pop : {}
    end
  end
end

# ActiveSupport's `HashWithIndifferentAccess`: String keys, read and written by a Symbol as well, nested Hashes
# converted, for a gem's code on untyped values (Doorkeeper's `request.parameters.values_at(:client_id, …)`).
unless defined?(ActiveSupport::HashWithIndifferentAccess)
  module ActiveSupport
    class HashWithIndifferentAccess < Hash
      # Rails' constructor: a Hash's entries and its defaults, or a plain default value.
      def initialize(constructor = nil)
        if constructor.respond_to?(:to_hash)
          super()
          update(constructor)
          hash = constructor.is_a?(Hash) ? constructor : constructor.to_hash
          self.default = hash.default if hash.default
          self.default_proc = hash.default_proc if hash.default_proc
        elsif constructor.nil?
          super()
        else
          super(constructor)
        end
      end

      # Rails' `extractable_options?`: an indifferent Hash is a trailing options Hash too.
      def extractable_options?
        true
      end

      def self.[](*args)
        new.merge!(Hash[*args])
      end

      alias regular_writer []=
      private :regular_writer

      def []=(key, value)
        super(convert_key(key), convert_value(value))
      end
      alias store []=

      def [](key)
        super(convert_key(key))
      end

      def fetch(key, *extras, &block)
        super(convert_key(key), *extras, &block)
      end

      def dig(key, *rest)
        super(convert_key(key), *rest)
      end

      def key?(key)
        super(convert_key(key))
      end
      alias include? key?
      alias has_key? key?
      alias member? key?

      # Not left to Hash's: MRI's `fetch_values` looks keys up in C, past this class's `fetch`.
      def fetch_values(*keys, &block)
        keys.map { |key| fetch(key, &block) }
      end

      def values_at(*keys)
        keys.map { |key| self[key] }
      end

      def delete(key)
        super(convert_key(key))
      end

      # Rails' `update`: a key already here takes the block's answer for its old and new values.
      def update(*others, &block)
        others.each do |other|
          # Not `to_hash` for another indifferent Hash: that rebuilds its nested ones, and a copy is shallow in Rails.
          indifferent = other.is_a?(HashWithIndifferentAccess)
          (indifferent ? other : other.to_hash).each_pair do |key, value|
            value = block.call(convert_key(key), self[key], value) if block && key?(key)
            # Not converted again from another indifferent Hash: its values are already, and a copy shares them.
            indifferent ? regular_writer(convert_key(key), value) : self[key] = value
          end
        end
        self
      end
      alias merge! update

      def merge(*others, &block)
        dup.update(*others, &block)
      end

      # Rails' `reverse_merge`: the defaults' keys converted first, so this Hash's own values win under either spelling.
      def reverse_merge(other_hash)
        self.class.new(other_hash).merge!(self)
      end
      alias with_defaults reverse_merge

      def reverse_merge!(other_hash)
        replace(reverse_merge(other_hash))
      end
      alias with_defaults! reverse_merge!

      def replace(other_hash)
        super(self.class.new(other_hash))
      end

      # Rails' filters and transforms: an indifferent Hash again, its keys written through `[]=`.
      def select(*args, &block)
        return to_enum(:select) unless block
        dup.tap { |hash| hash.select!(*args, &block) }
      end
      alias filter select

      def reject(*args, &block)
        return to_enum(:reject) unless block
        dup.tap { |hash| hash.reject!(*args, &block) }
      end

      def transform_values(&block)
        return to_enum(:transform_values) unless block
        dup.tap { |hash| hash.transform_values!(&block) }
      end

      # Rails 8.1's: the whole Hash transformed first and then taken in, so keys that trade places keep their values.
      def transform_keys(mapping = nil, &block)
        return to_enum(:transform_keys) if mapping.nil? && !block
        self.class.new(mapping ? super(mapping, &block) : super(&block))
      end

      def transform_keys!(mapping = nil, &block)
        return to_enum(:transform_keys!) if mapping.nil? && !block
        transformed = transform_keys(mapping, &block)
        transformed.default = default
        transformed.default_proc = default_proc if default_proc
        replace(transformed)
        self
      end

      # Rails' `symbolize_keys`: a plain Hash, since an indifferent one would write the Symbols back as Strings.
      def symbolize_keys
        to_hash.transform_keys { |key| key.respond_to?(:to_sym) ? key.to_sym : key }
      end
      alias to_options symbolize_keys

      def compact
        dup.tap(&:compact!)
      end

      def slice(*keys)
        self.class.new(super(*keys.map { |key| convert_key(key) }))
      end

      def except(*keys)
        self.class.new(super(*keys.map { |key| convert_key(key) }))
      end

      def dup
        self.class.new(self)
      end

      def with_indifferent_access
        dup
      end

      def to_hash
        out = {}
        out.default = default
        out.default_proc = default_proc if default_proc
        each { |key, value| out[key] = plain(value) }
        out
      end

      private

      # Rails' `to_hash` conversion: nested indifferent Hashes, in Arrays too, become plain ones.
      def plain(value)
        case value
        when HashWithIndifferentAccess then value.to_hash
        when Array then value.map { |item| plain(item) }
        else value
        end
      end

      def convert_key(key)
        key.is_a?(Symbol) ? key.name : key
      end

      def convert_value(value)
        case value
        when HashWithIndifferentAccess then value
        when Hash then self.class.new(value)
        when Array then value.map { |item| convert_value(item) }
        else value
        end
      end
    end
  end

  class Hash
    def with_indifferent_access
      ActiveSupport::HashWithIndifferentAccess.new(self)
    end
  end
end
