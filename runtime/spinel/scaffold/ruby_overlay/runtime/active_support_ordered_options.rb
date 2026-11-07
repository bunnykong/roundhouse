# frozen_string_literal: true

# `ActiveSupport::OrderedOptions`, ported from activesupport 8.1
# `lib/active_support/ordered_options.rb` (MIT license, Copyright (c)
# David Heinemeier Hansson). It is a Hash: a name that Hash does not
# define reads that key (`options.max_file_size`), `name=` writes it,
# and `name!` raises when the value is blank.
#
# Only the CRuby and JRuby trees run `config_for`, and only they ship
# this file. Rails' `_get` alias is left out: only its
# `InheritableOptions`, which is not ported, calls it.
module ActiveSupport
  class OrderedOptions < Hash
    def []=(key, value)
      super(key.to_sym, value)
    end

    def [](key)
      super(key.to_sym)
    end

    def dig(key, *identifiers)
      super(key.to_sym, *identifiers)
    end

    def method_missing(method, *args)
      if method.end_with?("=")
        self[method.name.chomp("=")] = args.first
      elsif method.end_with?("!")
        name_string = method.name.chomp("!")
        self[name_string].presence || raise(KeyError.new(":#{name_string} is blank"))
      else
        self[method.name]
      end
    end

    def respond_to_missing?(name, include_private)
      true
    end

    def extractable_options?
      true
    end

    def inspect
      "#<#{self.class.name} #{super}>"
    end
  end
end
