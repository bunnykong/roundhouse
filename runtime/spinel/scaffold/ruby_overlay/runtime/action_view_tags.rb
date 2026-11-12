# frozen_string_literal: true
# Rails 8.1's `ActionView::Helpers::Tags` for the collection inputs a gem's builder subclasses (simple_form's
# `CollectionRadioButtons < ActionView::Helpers::Tags::CollectionRadioButtons`), and the view-context helpers they
# call back on `@template_object`. CRuby overlay, beside `action_view_form_builder.rb`.
module ActionView
  class Base
    def field_id(object_name, method_name, *suffixes, index: nil, namespace: nil)
      object_name = object_name.model_name.singular if object_name.respond_to?(:model_name)
      sanitized_object_name = object_name.to_s.gsub(/\]\[|[^-a-zA-Z0-9:.]/, "_").delete_suffix("_")
      [namespace, (sanitized_object_name unless sanitized_object_name.empty?), (index unless sanitized_object_name.empty?),
       method_name.to_s.delete_suffix("?"), *suffixes].compact.join("_")
    end

    def field_name(object_name, method_name, *method_names, multiple: false, index: nil)
      names = method_names.map { |name| "[#{name}]" }.join
      if object_name.nil? || object_name.to_s.empty?
        "#{method_name}#{names}#{multiple ? "[]" : ""}"
      elsif index
        "#{object_name}[#{index}][#{method_name}]#{names}#{multiple ? "[]" : ""}"
      else
        "#{object_name}[#{method_name}]#{names}#{multiple ? "[]" : ""}"
      end
    end

    # Rails' FormHelper: each field the `Tags::*` class of its name renders.
    { text_field: "TextField", password_field: "PasswordField", hidden_field: "HiddenField", email_field: "EmailField",
      url_field: "UrlField", telephone_field: "TelField", search_field: "SearchField", color_field: "ColorField",
      number_field: "NumberField", range_field: "RangeField", date_field: "DateField", time_field: "TimeField",
      datetime_field: "DatetimeLocalField", datetime_local_field: "DatetimeLocalField", month_field: "MonthField",
      week_field: "WeekField", text_area: "TextArea" }.each do |helper, tag|
      define_method(helper) { |object_name, method, options = {}| Helpers::Tags.const_get(tag).new(object_name, method, self, options).render }
    end
    alias phone_field telephone_field
    alias textarea text_area

    def label(object_name, method, content_or_options = nil, options = nil, &block)
      Helpers::Tags::Label.new(object_name, method, self, content_or_options, options).render(&block)
    end

    def radio_button(object_name, method, tag_value, options = {})
      Helpers::Tags::RadioButton.new(object_name, method, self, tag_value, options).render
    end

    def checkbox(object_name, method, options = {}, checked_value = "1", unchecked_value = "0")
      Helpers::Tags::CheckBox.new(object_name, method, self, checked_value, unchecked_value, options).render
    end
    alias check_box checkbox

    def hidden_field_tag(name, value = nil, options = {})
      tag(:input, { "type" => "hidden", "name" => name, "id" => sanitize_to_id(name), "value" => value, "autocomplete" => "off" }.merge(Helpers::FormBuilder.stringify(options)))
    end

    def label_tag(name = nil, content_or_options = nil, options = nil, &block)
      if block && content_or_options.is_a?(Hash)
        options = content_or_options
      else
        options ||= {}
      end
      opts = Helpers::FormBuilder.stringify(options)
      opts["for"] = sanitize_to_id(name) unless name.nil? || name.to_s.empty? || opts.key?("for")
      content_tag(:label, block ? capture(&block) : (content_or_options.is_a?(Hash) ? nil : content_or_options) || ActiveSupport.humanize(name.to_s), opts)
    end

    private

    # Rails' `sanitize_to_id`: `]` dropped, each other non-id character an underscore.
    def sanitize_to_id(name)
      name.to_s.delete("]").tr("^-a-zA-Z0-9:.", "_")
    end

  end

  module Helpers
    module FormOptionsHelper
      private

      def option_html_attributes(element)
        element.is_a?(Array) ? element.select { |e| e.is_a?(Hash) }.reduce({}, :merge!) : {}
      end

      def value_for_collection(item, value)
        value.respond_to?(:call) ? value.call(item) : item.public_send(value)
      end
    end

    module Tags
      class Base
        def object
          @active_model_object ||= @object.respond_to?(:to_model) ? @object.to_model : @object
        end

        def initialize(object_name, method_name, template_object, options = {})
          @object_name, @method_name = object_name.to_s.dup, method_name.to_s.dup
          @template_object = template_object
          @object_name.sub!(/\[\]$/, "") || @object_name.sub!(/\[\]\]$/, "]")
          @object = retrieve_object(options.delete(:object))
          @skip_default_ids = options.delete(:skip_default_ids)
          @allow_method_names_outside_object = options.delete(:allow_method_names_outside_object)
          @options = options
          if Regexp.last_match
            @generate_indexed_names = true
            @auto_index = retrieve_autoindex(Regexp.last_match.pre_match)
          else
            @generate_indexed_names = false
            @auto_index = nil
          end
        end

        def render
          raise NotImplementedError, "Subclasses must implement a render method"
        end

        private

        # Rails' ActiveModelInstanceTag: a field of an attribute with errors runs `field_error_proc`, a hidden one not.
        def tag(name, options)
          html = @template_object.tag(name, options)
          options["type"] == "hidden" ? html : error_wrapping(html)
        end

        def content_tag(name, content, options)
          error_wrapping(@template_object.content_tag(name, content, options))
        end

        def error_wrapping(html_tag)
          return html_tag unless object_has_errors?
          @template_object.instance_exec(html_tag, self, &ActionView::Base.field_error_proc)
        end

        def error_message
          object.errors[@method_name]
        end

        def object_has_errors?
          object.respond_to?(:errors) && object.errors.respond_to?(:[]) && error_message.present?
        end

        def value_before_type_cast
          return unless object
          method_before_type_cast = @method_name + "_before_type_cast"
          if value_came_from_user? && object.respond_to?(method_before_type_cast)
            object.public_send(method_before_type_cast)
          else
            value
          end
        end

        def value_came_from_user?
          method_name = "#{@method_name}_came_from_user?"
          !object.respond_to?(method_name) || object.public_send(method_name)
        end

        def value
          return unless object
          if @allow_method_names_outside_object
            object.public_send @method_name if object.respond_to?(@method_name)
          else
            object.public_send @method_name
          end
        end

        def retrieve_object(object)
          if object
            object
          elsif @template_object.instance_variable_defined?("@#{@object_name}")
            @template_object.instance_variable_get("@#{@object_name}")
          end
        rescue NameError
          nil
        end

        def retrieve_autoindex(pre_match)
          object = self.object || @template_object.instance_variable_get("@#{pre_match}")
          raise ArgumentError, "object[] naming but object param and @object var don't exist or don't respond to to_param: #{object.inspect}" unless object.respond_to?(:to_param)
          object.to_param
        end

        def add_default_name_and_field_for_value(tag_value, options, field = "id")
          if tag_value.nil?
            add_default_name_and_field(options, field)
          else
            specified_field = options[field]
            add_default_name_and_field(options, field)
            options[field] += "_#{sanitized_value(tag_value)}" if (specified_field.nil? || specified_field.to_s.strip.empty?) && !options[field].to_s.strip.empty?
          end
        end
        alias add_default_name_and_id_for_value add_default_name_and_field_for_value

        def add_default_name_and_field(options, field = "id")
          index = name_and_id_index(options)
          options["name"] = options.fetch("name") { tag_name(options["multiple"], index) }
          return unless generate_ids?
          options[field] = options.fetch(field) { tag_id(index, options.delete("namespace")) }
          if (namespace = options.delete("namespace"))
            options[field] = options[field] ? "#{namespace}_#{options[field]}" : namespace
          end
        end
        alias add_default_name_and_id add_default_name_and_field

        def tag_name(multiple = false, index = nil)
          @template_object.field_name(@object_name, sanitized_method_name, multiple: multiple, index: index)
        end

        def tag_id(index = nil, namespace = nil)
          @template_object.field_id(@object_name, @method_name, index: index, namespace: namespace)
        end

        def sanitized_method_name
          @sanitized_method_name ||= @method_name.delete_suffix("?")
        end

        def sanitized_value(value)
          value.to_s.gsub(/[\s.]/, "_").gsub(/[^-[[:word:]]]/, "").downcase
        end

        def name_and_id_index(options)
          if options.key?("index")
            options.delete("index") || ""
          elsif @generate_indexed_names
            @auto_index || ""
          end
        end

        def generate_ids?
          !@skip_default_ids
        end
      end

      # Rails' `Tags::Placeholderable`: `placeholder: true` is the attribute's human name.
      module Placeholderable
        def initialize(*)
          super
          tag_value = @options[:placeholder]
          return unless tag_value
          @options[:placeholder] = tag_value.is_a?(String) ? tag_value : Translator.new(object, @object_name, tag_value.is_a?(TrueClass) ? @method_name : "#{@method_name}.#{tag_value}").translate
        end
      end

      # Rails' `Tags::Translator`, by the model's `human_attribute_name`; the `helpers.label` I18n lookup is not modeled.
      class Translator
        def initialize(object, object_name, method_and_value)
          @model = object.respond_to?(:to_model) ? object.to_model : nil
          @method_and_value = method_and_value
        end

        def translate
          name = @model.class.human_attribute_name(@method_and_value) if @model && @model.class.respond_to?(:human_attribute_name)
          name || ActiveSupport.humanize(@method_and_value.to_s)
        end
      end

      class TextField < Base
        include Placeholderable

        def self.field_type
          @field_type ||= name.split("::").last.sub("Field", "").downcase
        end

        def render
          options = @options.transform_keys(&:to_s)
          options["size"] = options["maxlength"] unless options.key?("size")
          options["type"] ||= field_type
          options["value"] = options.fetch("value") { value_before_type_cast } unless field_type == "file"
          add_default_name_and_field(options)
          tag("input", options)
        end

        private

        def field_type
          self.class.field_type
        end
      end

      class PasswordField < TextField
        def render
          @options = { value: nil }.merge!(@options)
          super
        end
      end

      class HiddenField < TextField
        def render
          @options = { autocomplete: "off" }.merge(@options)
          super
        end
      end

      class EmailField < TextField; end
      class UrlField < TextField; end
      class TelField < TextField; end
      class SearchField < TextField; end
      class ColorField < TextField; end

      class NumberField < TextField
        def render
          options = @options.transform_keys(&:to_s)
          if (range = options.delete("in") || options.delete("within"))
            options.update("min" => range.min, "max" => range.max)
          end
          @options = options
          super
        end
      end

      class RangeField < NumberField; end

      class DatetimeField < TextField
        def render
          options = @options.transform_keys(&:to_s)
          options["value"] = datetime_value(options.fetch("value") { value })
          options["min"] = format_datetime(parse_datetime(options["min"]))
          options["max"] = format_datetime(parse_datetime(options["max"]))
          @options = options
          super
        end

        private

        def datetime_value(value)
          value.is_a?(String) ? value : format_datetime(value)
        end

        def parse_datetime(value)
          value.is_a?(String) ? (DateTime.parse(value) rescue nil) : value
        end
      end

      class DateField < DatetimeField
        private def format_datetime(value) = value&.strftime("%Y-%m-%d")
      end

      class DatetimeLocalField < DatetimeField
        def self.field_type = "datetime-local"

        def initialize(object_name, method_name, template_object, options = {})
          @include_seconds = options.delete(:include_seconds) { true }
          super
        end

        private def format_datetime(value) = value&.strftime(@include_seconds ? "%Y-%m-%dT%T" : "%Y-%m-%dT%H:%M")
      end

      class TimeField < DatetimeField
        def initialize(object_name, method_name, template_object, options = {})
          @include_seconds = options.delete(:include_seconds) { true }
          super
        end

        private def format_datetime(value) = value&.strftime(@include_seconds ? "%T.%L" : "%H:%M")
      end

      class MonthField < DatetimeField
        private def format_datetime(value) = value&.strftime("%Y-%m")
      end

      class WeekField < DatetimeField
        private def format_datetime(value) = value&.strftime("%G-W%V")
      end

      class TextArea < Base
        include Placeholderable

        def render
          options = @options.transform_keys(&:to_s)
          add_default_name_and_field(options)
          if (size = options.delete("size")) && size.respond_to?(:split)
            options["cols"], options["rows"] = size.split("x")
          end
          content_tag("textarea", options.delete("value") { value_before_type_cast }, options)
        end
      end

      module Checkable
        def input_checked?(options)
          if options.key?("checked")
            checked = options.delete("checked")
            checked == true || checked == "checked"
          else
            checked?(value)
          end
        end
      end

      class CheckBox < Base
        include Checkable

        def initialize(object_name, method_name, template_object, checked_value, unchecked_value, options)
          @checked_value = checked_value
          @unchecked_value = unchecked_value
          super(object_name, method_name, template_object, options)
        end

        def render
          options = @options.transform_keys(&:to_s)
          options["type"] = "checkbox"
          options["value"] = @checked_value
          options["checked"] = "checked" if input_checked?(options)
          if options["multiple"]
            add_default_name_and_field_for_value(@checked_value, options)
            options.delete("multiple")
          else
            add_default_name_and_field(options)
          end
          include_hidden = options.key?("include_hidden") ? options.delete("include_hidden") : true
          checkbox = tag("input", options)
          include_hidden ? (hidden_field_for_checkbox(options) + checkbox).html_safe : checkbox
        end

        private

        def checked?(value)
          case value
          when TrueClass, FalseClass then value == !!@checked_value
          when NilClass then false
          when String then value == @checked_value
          else
            value.respond_to?(:include?) ? value.include?(@checked_value) : value.to_i == @checked_value.to_i
          end
        end

        def hidden_field_for_checkbox(options)
          return "".html_safe unless @unchecked_value
          tag("input", options.slice("name", "disabled", "form").merge!("type" => "hidden", "value" => @unchecked_value, "autocomplete" => "off"))
        end
      end

      class RadioButton < Base
        include Checkable

        def initialize(object_name, method_name, template_object, tag_value, options)
          @tag_value = tag_value
          super(object_name, method_name, template_object, options)
        end

        def render
          options = @options.transform_keys(&:to_s)
          options["type"] = "radio"
          options["value"] = @tag_value
          options["checked"] = "checked" if input_checked?(options)
          add_default_name_and_field_for_value(@tag_value, options)
          tag("input", options)
        end

        private

        def checked?(value)
          value.to_s == @tag_value.to_s
        end
      end

      class Label < Base
        class LabelBuilder
          attr_reader :object

          def initialize(template_object, object_name, method_name, object, tag_value)
            @template_object = template_object
            @object_name = object_name
            @method_name = method_name
            @object = object
            @tag_value = tag_value
          end

          def translation
            Translator.new(object, @object_name, @tag_value.present? ? "#{@method_name}.#{@tag_value}" : @method_name).translate
          end

          def to_s
            translation
          end
        end

        def initialize(object_name, method_name, template_object, content_or_options = nil, options = nil)
          options ||= {}
          if content_or_options.is_a?(Hash)
            options = options.merge(content_or_options)
            @content = nil
          else
            @content = content_or_options
          end
          super(object_name, method_name, template_object, options)
        end

        def render(&block)
          options = @options.transform_keys(&:to_s)
          tag_value = options.delete("value")
          add_default_name_and_field_for_value(tag_value, options, "for")
          options.delete("index")
          options.delete("name")
          options.delete("namespace")
          builder = LabelBuilder.new(@template_object, @object_name, @method_name, object, tag_value)
          content = if block
            @template_object.capture(builder, &block)
          elsif @content.present?
            @content.to_s
          else
            builder.translation
          end
          error_wrapping(@template_object.label_tag(options["for"], content, options))
        end
      end

      module CollectionHelpers
        class Builder
          attr_reader :object, :text, :value

          def initialize(template_object, object_name, method_name, object, sanitized_attribute_name, text, value, input_html_options)
            @template_object = template_object
            @object_name = object_name
            @method_name = method_name
            @object = object
            @sanitized_attribute_name = sanitized_attribute_name
            @text = text
            @value = value
            @input_html_options = input_html_options
          end

          def label(label_html_options = {}, &block)
            html_options = @input_html_options.slice(:index, :namespace).merge(label_html_options)
            html_options[:for] ||= @input_html_options[:id] if @input_html_options[:id]
            @template_object.label(@object_name, @sanitized_attribute_name, @text, html_options, &block)
          end
        end

        def initialize(object_name, method_name, template_object, collection, value_method, text_method, options, html_options)
          @collection = collection
          @value_method = value_method
          @text_method = text_method
          @html_options = html_options
          super(object_name, method_name, template_object, options)
        end

        private

        def instantiate_builder(builder_class, item, value, text, html_options)
          builder_class.new(@template_object, @object_name, @method_name, item, sanitize_attribute_name(value), text, value, html_options)
        end

        def default_html_options_for_collection(item, value)
          html_options = @html_options.dup
          [:checked, :selected, :disabled, :readonly].each do |option|
            current_value = @options[option]
            next if current_value.nil?
            accept = current_value.respond_to?(:call) ? current_value.call(item) : Array(current_value).map(&:to_s).include?(value.to_s)
            if accept
              html_options[option] = true
            elsif option == :checked
              html_options[option] = false
            end
          end
          html_options[:object] = @object
          html_options
        end

        def sanitize_attribute_name(value)
          sanitized = sanitized_value(value)
          sanitized.empty? ? sanitized_method_name.dup : "#{sanitized_method_name}_#{sanitized}"
        end

        def render_collection
          @collection.map do |item|
            value = value_for_collection(item, @value_method)
            text = value_for_collection(item, @text_method)
            yield item, value, text, default_html_options_for_collection(item, value).merge(option_html_attributes(item))
          end.join.html_safe
        end

        def render_collection_for(builder_class, &block)
          options = @options.transform_keys(&:to_s)
          rendered_collection = render_collection do |item, value, text, default_html_options|
            builder = instantiate_builder(builder_class, item, value, text, default_html_options)
            block ? @template_object.capture(builder, &block) : render_component(builder)
          end
          options.fetch("include_hidden", true) ? hidden_field + rendered_collection : rendered_collection
        end

        def hidden_field
          @template_object.hidden_field_tag(@html_options[:name] || hidden_field_name, "", { id: nil, form: @html_options[:form] })
        end

        def hidden_field_name
          tag_name(false, @options[:index]).to_s
        end
      end

      class CollectionRadioButtons < Base
        include CollectionHelpers
        include FormOptionsHelper

        class RadioButtonBuilder < Builder
          def radio_button(extra_html_options = {})
            html_options = extra_html_options.merge(@input_html_options)
            html_options[:skip_default_ids] = false
            @template_object.radio_button(@object_name, @method_name, @value, html_options)
          end
        end

        def render(&block)
          render_collection_for(RadioButtonBuilder, &block)
        end

        private

        def render_component(builder)
          builder.radio_button + builder.label
        end
      end

      class CollectionCheckBoxes < Base
        include CollectionHelpers
        include FormOptionsHelper

        class CheckBoxBuilder < Builder
          def checkbox(extra_html_options = {})
            html_options = extra_html_options.merge(@input_html_options)
            html_options[:multiple] = true
            html_options[:skip_default_ids] = false
            @template_object.checkbox(@object_name, @method_name, html_options, @value, nil)
          end
          alias check_box checkbox
        end

        def render(&block)
          render_collection_for(CheckBoxBuilder, &block)
        end

        private

        def render_component(builder)
          builder.checkbox + builder.label
        end

        def hidden_field_name
          "#{super}[]"
        end
      end
    end
  end
end
