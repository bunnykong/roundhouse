# frozen_string_literal: true

# The view context Rails instantiates for a template. Roundhouse still
# macro-expands `form_with` into the accumulator (the block binds a
# typed builder the runtime does not reconstruct), but each expanded
# site begins by calling this instance method. A fragment-cache hit
# never walks that site, so it never enters `form_with`.
#
# That is the entry `ActionView::Base.any_instance.expects(:form_with)`
# observes. The method returns nil; the markup that follows in the
# expansion is the form. A body that also emitted the tags would
# double them.
module ActionView
  class Base
    def form_with
      nil
    end
  end
end

module ActionView
  # Rails 8.1's `ActionView::OutputBuffer`: a template's markup, a String escaped as it is appended unless html_safe
  # (`ERB::Util.unwrapped_html_escape`), raw through `safe_concat`. A component's render (view_component's
  # `render_in`) starts one.
  class OutputBuffer
    def initialize(buffer = "")
      @raw_buffer = +""
      @raw_buffer << buffer
    end

    def to_s
      @raw_buffer
    end

    def to_str
      @raw_buffer.dup
    end

    def length
      @raw_buffer.length
    end

    def empty?
      @raw_buffer.empty?
    end

    def html_safe?
      true
    end

    def <<(value)
      @raw_buffer << ActionView::ViewHelpers.html_escape(value) unless value.nil?
      self
    end

    def safe_concat(value)
      @raw_buffer << value
      self
    end
  end

  # What a view's `lookup_context` answers a component: no request variant (`+phone`) and the HTML format, the
  # template a request without either renders.
  class LookupContext
    def variants
      []
    end

    def formats
      [:html]
    end
  end

  class Base
    def lookup_context
      @lookup_context ||= LookupContext.new
    end

    # Rails' renderer and content_for flow: none is modelled here, and a component reads them only through helpers
    # this runtime does not have (older Rails' path helpers, `content_for` from a component).
    def view_renderer
      nil
    end

    def view_flow
      nil
    end
  end
end
