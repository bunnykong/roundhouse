# frozen_string_literal: true
# Rails' `ActionView::Helpers::FormBuilder` and the view-context helpers it calls through `@template`, at request
# time, for a builder the form lowering does not expand: an app's subclass (`form_with(builder: MoneyFormBuilder)`)
# or a gem's (simple_form's). The lowering still writes a stock builder's fields as markup; a builder method of
# another class runs here, and what it calls back (`text_field`, `label`, `@template.content_tag`) answers the
# markup Rails 8.1 does, marked html_safe so the template writes it as it is. CRuby overlay: the record is reached
# through `public_send` by the field's name, which the typed runtime does not carry.
module ActionView
  class Base
    # Rails' default: the field and its label boxed, run on the view (`instance_exec`) with the field's tag.
    @field_error_proc = proc { |html_tag, _instance| "<div class=\"field_with_errors\">#{html_tag}</div>".html_safe }

    class << self
      attr_accessor :field_error_proc
    end

    # The request's controller, which Rails' view context holds (simple_form reads `template.controller.action_name`).
    def controller
      ActionController::Current.controller
    end

    def request
      ActionController::Current.request
    end

    # Rails' `form_for`: the record's name, id and class, its route, and the method its persistence says, as
    # `form_with(model:)` writes them; the block's markup between the form tag and its end.
    def form_for(record, options = {}, &block)
      raise ArgumentError, "Missing block" unless block
      html = (options[:html] || {}).dup
      if record.is_a?(String) || record.is_a?(Symbol)
        object = nil
        object_name = record
      else
        object = record.is_a?(Array) ? record.last : record
        object = object.to_model if object.respond_to?(:to_model)
        object_name = options[:as] || object.class.model_name.param_key
        as = options[:as]
        action, method = object.respond_to?(:persisted?) && object.persisted? ? [:edit, :patch] : [:new, :post]
        defaults = { class: as ? "#{action}_#{as}" : ActionView::ViewHelpers.dom_class(object, action),
                     id: (as ? [options[:namespace], action, as] : [options[:namespace], ActionView::ViewHelpers.dom_id(object, action)]).compact.join("_") }
        defaults.delete(:id) if defaults[:id].empty?
        html = defaults.merge(html)
        options = options.merge(url: options[:url] || polymorphic_path(record), method: options[:method] || method)
      end
      # Not answered here: the routes resolve a controller/action Hash at transpile time, not on the view.
      raise NotImplementedError, "form_for with a url: Hash (#{options[:url].inspect}) is not modeled" if options[:url].is_a?(Hash)
      builder_class = options[:builder] || Helpers::FormBuilder
      builder_options = options.except(:builder, :html, :url, :method).merge(skip_default_ids: false, allow_method_names_outside_object: options.fetch(:allow_method_names_outside_object, false))
      builder = builder_class.new(object_name, object, self, builder_options)
      body = capture(builder, &block)
      method = (html.delete(:method) || options[:method] || :post).to_s
      html[:enctype] = "multipart/form-data" if html.delete(:multipart) || builder.multipart?
      html[:action] = options[:url].is_a?(String) ? options[:url] : polymorphic_path(options[:url])
      html[:"accept-charset"] = "UTF-8"
      html[:method] = method == "get" ? "get" : "post"
      extra = method == "get" ? "" : ActionView::ViewHelpers.method_override_input(method) + token_tag(options.key?(:authenticity_token) ? options[:authenticity_token] : html.delete(:authenticity_token))
      "<form#{ActionView::ViewHelpers.render_attrs(Helpers::FormBuilder.tag_options(html))}>#{extra}#{body}</form>".html_safe
    end

    # Rails' `token_tag`: none for `authenticity_token: false`, the given token, else the session's.
    def token_tag(token)
      return "" if token == false
      default = ActionView::ViewHelpers.csrf_token_hidden_input
      return default if token.nil? || token == true || default.empty?
      tag(:input, "type" => "hidden", "name" => "authenticity_token", "value" => token, "autocomplete" => "off")
    end

    # Rails' `polymorphic_path` for a record, a class or a nesting of them, by the app's route helpers: the
    # collection's for a new record or a class, the member's for a persisted one.
    def polymorphic_path(record_or_array)
      parts = Array(record_or_array).compact
      last = parts.last
      member = !last.is_a?(Class) && !last.is_a?(Symbol) && !last.is_a?(String) && last.respond_to?(:persisted?) && last.persisted?
      args = []
      names = parts.each_with_index.map do |part, i|
        next part.to_s if part.is_a?(Symbol) || part.is_a?(String)
        model = part.is_a?(Class) ? part.model_name : part.class.model_name
        if i < parts.size - 1 || member
          args << part.to_param
          model.singular_route_key
        else
          model.route_key
        end
      end
      RouteHelpers.public_send(:"#{names.join("_")}_path", *args)
    end

    # Rails' `content_tag(name, content = nil, options = nil, &block)`.
    def content_tag(name, content_or_options = nil, options = nil, &block)
      if block
        options = content_or_options if content_or_options.is_a?(Hash)
        content = capture(&block)
      else
        content = content_or_options
      end
      ActionView::Helpers::FormBuilder.tag_html(name, content, options)
    end

    # Rails' `capture`: the block's value, a String escaped unless it is html_safe.
    def capture(*args)
      value = yield(*args)
      return nil unless value.is_a?(String)
      ActionView::ViewHelpers.html_escape(value).html_safe
    end

    def safe_join(array, sep = "")
      array.flatten.map { |item| ActionView::ViewHelpers.html_escape(item.to_s) }.join(ActionView::ViewHelpers.html_escape(sep.to_s)).html_safe
    end

    # Rails' `tag(name, options)`: a void element, `<br />`.
    def tag(name, options = nil)
      "<#{ActionView::ViewHelpers.content_tag_name(name.to_s)}#{ActionView::ViewHelpers.render_attrs(Helpers::FormBuilder.tag_options(options))} />".html_safe
    end

    # Rails' `submit_tag` and `set_default_disable_with`: the caption disables the button, under the `data` given.
    def submit_tag(value = "Save changes", options = {})
      opts = { "type" => "submit", "name" => "commit", "value" => value }.merge(ActionView::Helpers::FormBuilder.stringify(options))
      data = (opts["data"] || {}).to_h.transform_keys(&:to_s)
      if opts["data-disable-with"] == false || data["disable_with"] == false
        data.delete("disable_with")
      else
        data["disable_with"] = opts["data-disable-with"] || data["disable_with"] || value.to_s
      end
      opts.delete("data-disable-with")
      opts["data"] = data unless data.empty? && !opts.key?("data")
      tag(:input, opts)
    end

    # Rails' `button_tag`: an options Hash alone is the options, and also the caption, as Rails renders it.
    def button_tag(content_or_options = nil, options = nil, &block)
      options = content_or_options.is_a?(Hash) ? content_or_options : (options || {})
      opts = { "name" => "button", "type" => "submit" }.merge(ActionView::Helpers::FormBuilder.stringify(options))
      block ? content_tag(:button, opts, &block) : content_tag(:button, content_or_options || "Button", opts)
    end
  end

  module Helpers
    # Rails 8.1's FormBuilder: each field is the view's helper of the same name, which renders the `Tags::*` class
    # (`action_view_tags.rb`) with the builder's object and default options.
    class FormBuilder
      attr_reader :object, :object_name, :options, :index

      def initialize(object_name, object, template, options)
        @object_name = object_name
        @object = object
        @template = template
        @options = options
        @default_options = options ? options.slice(:index, :namespace, :skip_default_ids, :allow_method_names_outside_object) : {}
        @default_html_options = @default_options.except(:skip_default_ids, :allow_method_names_outside_object)
        @multipart = nil
        @index = options[:index] || options[:child_index]
      end

      def multipart?
        @multipart
      end

      # Rails' `tag_option`: a list of values is one attribute, space-separated.
      def self.tag_options(options)
        (options || {}).to_h.transform_values { |v| v.is_a?(Array) ? v.flatten.compact.join(" ") : v }
      end

      def self.stringify(options)
        (options || {}).to_h.transform_keys(&:to_s)
      end

      def self.tag_html(name, content, options)
        n = ActionView::ViewHelpers.content_tag_name(name.to_s)
        body = ActionView::ViewHelpers.html_escape(content.to_s)
        "<#{n}#{ActionView::ViewHelpers.render_attrs(tag_options(options))}>#{ActionView::ViewHelpers.content_tag_pre(n)}#{body}</#{n}>".html_safe
      end

      def field_id(method, *suffixes, namespace: @options[:namespace], index: @options[:index])
        @template.field_id(@object_name, method, *suffixes, namespace: namespace, index: index)
      end

      def field_name(method, *methods, multiple: false, index: @options[:index])
        @template.field_name(@options.fetch(:as) { @object_name }, method, *methods, index: index, multiple: multiple)
      end

      %i[text_field password_field email_field url_field telephone_field phone_field number_field range_field search_field
         color_field date_field time_field datetime_field datetime_local_field month_field week_field text_area textarea].each do |selector|
        define_method(selector) { |method, options = {}| @template.public_send(selector, @object_name, method, objectify_options(options)) }
      end

      def hidden_field(method, options = {})
        @template.hidden_field(@object_name, method, objectify_options(options))
      end

      def checkbox(method, options = {}, checked_value = "1", unchecked_value = "0")
        @template.checkbox(@object_name, method, objectify_options(options), checked_value, unchecked_value)
      end
      alias check_box checkbox

      def radio_button(method, tag_value, options = {})
        @template.radio_button(@object_name, method, tag_value, objectify_options(options))
      end

      def label(method, text = nil, options = {}, &block)
        @template.label(@object_name, method, text, objectify_options(options), &block)
      end

      def submit(value = nil, options = {})
        value, options = nil, value if value.is_a?(Hash)
        @template.submit_tag(value || submit_default_value, options)
      end

      def button(value = nil, options = {}, &block)
        case value
        when Hash
          value, options = nil, value
        when Symbol
          value, options = nil, { name: field_name(value), id: field_id(value) }.merge!(options.to_h)
        end
        value ||= submit_default_value
        value = @template.capture { yield(value) } if block
        formmethod = options[:formmethod]
        if formmethod && !formmethod.to_s.empty? && !/post|get/i.match?(formmethod.to_s) && !options.key?(:name) && !options.key?(:value)
          options = options.merge(formmethod: :post, name: "_method", value: formmethod)
        end
        @template.button_tag(value, options)
      end

      private

      def objectify_options(options)
        result = @default_options.merge(options)
        result[:object] = @object
        result
      end

      def convert_to_model(object)
        object.respond_to?(:to_model) ? object.to_model : object
      end

      # Rails' `submit_default_value`: Create or Update, and the model's human name.
      def submit_default_value
        object = convert_to_model(@object)
        key = object ? (object.respond_to?(:persisted?) && object.persisted? ? "Update" : "Create") : "Submit"
        model = if object.respond_to?(:model_name)
          object.model_name.human
        elsif object.class.respond_to?(:model_name)
          object.class.model_name.human
        else
          ActiveSupport.humanize(@object_name.to_s)
        end
        "#{key} #{model}"
      end
    end
  end
end
