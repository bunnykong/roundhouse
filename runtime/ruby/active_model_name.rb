# frozen_string_literal: true
# `ActiveModel::Name`, what `Model.model_name` answers: the class's name and the forms Rails derives from it, each
# written in by the model lowering (`schema::push_schema_methods`), `human` from the app's locales
# (`activerecord.models.<i18n_key>`, else the element humanized).
module ActiveModel
  class Name
    def initialize(name, human, element, singular, plural, i18n_key)
      @name = name
      @human = human
      @element = element
      @singular = singular
      @plural = plural
      @i18n_key = i18n_key
    end

    def name
      @name
    end

    def to_s
      @name
    end

    def human(options = {})
      @human
    end

    def element
      @element
    end

    def singular
      @singular
    end

    def plural
      @plural
    end

    def param_key
      @singular
    end

    def route_key
      @plural
    end

    def singular_route_key
      @singular
    end

    def i18n_key
      @i18n_key
    end
  end
end
