pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .write("app/models/document_base.rb", r#"
class DocumentBase
  def self.label(value = nil)
    value ? @label = value : @label
  end
end
"#)
        .write("app/models/labelled_document.rb", r#"
class LabelledDocument < DocumentBase
  label "first"
  FIRST = label
  self.label "sample"
  LAST = label
  def self.value = label
end
class UnlabelledDocument < DocumentBase
end
"#)
        .write("app/models/label_methods.rb", r#"
module LabelMethods
  def label(value = nil)
    value ? @label = value : @label
  end
end
"#)
        .write("app/models/extended_base.rb", r#"
class ExtendedBase
  extend LabelMethods
end
"#)
        .write("app/models/extended_document.rb", r#"
class ExtendedDocument < ExtendedBase
  label "extended"
  def self.value = label
end
"#)
}

pub fn settings_overlay() -> super::emit_and_run::Overlay {
    overlay()
        .write("app/models/settings_base.rb", r#"
class SettingsBase
  class_attribute :setting, default: "parent"
end
"#)
        .write("app/models/settings_child.rb", r#"
class SettingsChild < SettingsBase
  self.setting = "child"
end
"#)
}

pub const ASSERTIONS: &str = r#"
require_relative "app/models/labelled_document"
require_relative "app/models/extended_document"
require_relative "app/models/settings_child"
got = [LabelledDocument::FIRST, LabelledDocument::LAST, LabelledDocument.value,
       UnlabelledDocument.label, DocumentBase.label, ExtendedDocument.value, ExtendedBase.label]
raise got.inspect unless got == ["first", "sample", "sample", nil, nil, "extended", nil]
raise unless SettingsBase.setting == "parent" && SettingsChild.setting == "child"
puts "ordinary class body calls passed"
"#;
