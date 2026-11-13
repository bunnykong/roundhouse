# frozen_string_literal: true
# ActionMailer runtime — the delivery-capture slice.
#
# Rails' mailer machinery is method_missing at both ends (class-side
# `BanNotification.notify(...)` proxies to `new.notify(...)`, and
# delivery routes through delivery-method plugins). This runtime keeps
# the statically-resolvable core: `mail(opts)` builds a Message value
# object from the header options, and `deliver_now`/`deliver_later`
# append it to `ActionMailer::Base.deliveries` — Rails' :test delivery
# method, which is also what emitted tests will assert against. No
# SMTP; single-process apps that need real delivery get it as a
# per-target adapter later.
#
# The class-side call idiom is grounded at emit
# (`apply_mailer_class_side_lowering` synthesizes `def self.notify` →
# `new.notify(...)` wrappers on mailer classes), so nothing here is
# reflective.
#
# A mailer action's templates are rendered at compile time into the
# parts `mail` receives (`lower::mailer_templates`), with the class's
# `default` headers merged under the action's own.
module ActionMailer
  # One built mail. Header values are stored verbatim from the mail()
  # opts — raw SQL-style honesty: the corpus interpolates arbitrary
  # strings here, and nothing downstream needs them re-typed.
  # One part of a multipart message: its rendered template.
  class Part
    def initialize(body, mime)
      @body = body
      @mime = mime
    end

    def body
      @body
    end

    def decoded
      @body
    end

    def mime_type
      @mime
    end
  end

  # One header of a built mail, as `Mail::Message#[]` answers it.
  class Header
    def initialize(name, value)
      @name = name
      @value = value
    end

    def name
      @name
    end

    def value
      @value
    end
  end

  # One built mail, as ActionMailer's `Mail::Message` answers it: address
  # headers as Arrays of bare addresses, and with both a text and an html
  # template, a multipart/alternative whose own body is empty.
  class Message
    def initialize(opts, text_body = nil, html_body = nil)
      @to = Message.addresses(opts[:to])
      @from = Message.addresses(opts[:from])
      @cc = Message.addresses(opts[:cc])
      @bcc = Message.addresses(opts[:bcc])
      @reply_to = Message.addresses(opts[:reply_to])
      @subject = opts[:subject]
      @headers = {}
      @mailer = nil
      @null = false
      @perform_deliveries = true
      @text_body = text_body
      @html_body = html_body
      given = opts[:body]
      @body = if !text_body.nil? && !html_body.nil?
        ""
      elsif !text_body.nil?
        text_body
      elsif !html_body.nil?
        html_body
      else
        given.nil? ? "" : given.to_s
      end
    end

    # `"Name <a@example.com>"` → `"a@example.com"`; nil stays nil.
    def self.addresses(value)
      return nil if value.nil?
      list = value.is_a?(Array) ? value : [value]
      list.map do |item|
        text = item.to_s
        open = text.index("<")
        close = text.index(">")
        open.nil? || close.nil? || close < open ? text.strip : text[open + 1, close - open - 1].to_s
      end
    end

    def to
      @to
    end

    def from
      @from
    end

    def cc
      @cc
    end

    def bcc
      @bcc
    end

    def reply_to
      @reply_to
    end

    def subject
      @subject
    end

    # Headers a mailer added (`headers("X-Priority" => "1")`), by name.
    def headers
      @headers
    end

    def add_header(name, value)
      @headers[name] = value
      value
    end

    # `message["To"]`: an added header, or an address header `mail` set, by
    # name in any case; nil when the mail has none. The subject is
    # `subject` (its value is stored as given, untyped).
    def [](name)
      key = name.downcase
      found = nil
      @headers.each { |given, value| found = Header.new(given, value) if found.nil? && given.downcase == key }
      return found unless found.nil?
      value = case key
      when "to" then Message.joined(@to)
      when "from" then Message.joined(@from)
      when "cc" then Message.joined(@cc)
      when "bcc" then Message.joined(@bcc)
      when "reply-to" then Message.joined(@reply_to)
      end
      value.nil? ? nil : Header.new(name, value)
    end

    def self.joined(list)
      list.nil? ? nil : list.join(", ")
    end

    def body
      @body
    end

    def multipart?
      !@text_body.nil? && !@html_body.nil?
    end

    def text_part
      multipart? ? Part.new(@text_body.to_s, "text/plain") : nil
    end

    def html_part
      multipart? ? Part.new(@html_body.to_s, "text/html") : nil
    end

    # Rails' :test delivery semantics — collect, don't transmit. Both
    # return the message so `X.notify(...).deliver_now` chains stay
    # value-shaped. `deliver_later` has no queue in a single-process
    # runtime; immediate collection is the honest equivalent.
    #
    # A mail a mailer built is delivered through it, which runs its
    # delivery callbacks (`before_deliver`, …) around `__push`.
    def deliver_now
      mailer = @mailer
      mailer.nil? ? __push : mailer.__deliver(self)
    end

    def deliver_later
      deliver_now
    end

    # Collects the mail; a null one (`NullMail`: an action that built no
    # mail) is not collected and answers nil.
    def __push
      return nil if @null
      ActionMailer::Base.deliveries.push(self) if @perform_deliveries
      self
    end

    # Mail's switch: a mail an after_action turns off still runs its
    # delivery callbacks, and is not sent (lobsters' blocklist check).
    def perform_deliveries
      @perform_deliveries
    end

    def perform_deliveries=(value)
      @perform_deliveries = value
    end

    def __link(mailer)
      @mailer = mailer
      self
    end

    def __nullify
      @null = true
      self
    end
  end

  class Base
    # Class-body DSL (`default from: ...`) — accepted and ignored when
    # an ingest ever carries it through (today it's dropped upstream).
    def self.default(opts = nil)
      nil
    end

    # ActionMailer's own defaults (`ActionMailer::Base.default_params`); a mailer with `default` entries answers them
    # merged over these (`lower::mailer_templates`).
    def self.base_default_params
      { mime_version: "1.0", charset: "UTF-8", content_type: "text/plain", parts_order: ["text/plain", "text/enriched", "text/html"] }
    end

    def default_params
      ActionMailer::Base.base_default_params
    end

    # The :test-style delivery log. Lazy-init on first read (top-level
    # assignment at require time is a load-order trap under AOT).
    def self.deliveries
      ActionMailer::Base.__deliveries
    end

    # Not each mailer's own: Rails' test delivery log is one, which every mailer's `deliveries` answers.
    def self.__deliveries
      @deliveries ||= []
    end

    # The mail is kept as `message`, for the after callbacks; headers
    # given before it was built are added to it.
    def mail(opts = nil, text_body = nil, html_body = nil)
      current = @_message
      # Not rebuilt by a bare `mail` once the action built one: Rails answers that mail, which an after_action reads (lobsters' blocklist check).
      return current if opts.nil? && current
      built = Message.new(opts || {}, text_body, html_body).__link(self)
      __headers.each { |name, value| built.add_header(name, value) }
      @_message = built
      built
    end

    def message
      @_message
    end

    # What an action that built no mail answers, as Rails' `NullMail`:
    # delivering it runs the delivery callbacks and sends nothing.
    def __null_mail
      built = Message.new({}, nil, nil).__link(self).__nullify
      @_message = built
      built
    end

    # Delivers `mail`; a mailer with delivery callbacks overrides it
    # (`ingest::mailer_callbacks`).
    def __deliver(mail)
      mail.__push
    end

    # The action this mailer runs, which the generated callback chain sets
    # (`ingest::mailer_callbacks`).
    def action_name
      (@_action_name || @_called_action).to_s
    end

    # The action a mailer whose `mail` looks its template up runs, the
    # outermost: what `action_name` answers without a callback chain
    # (`lower::mailer_templates`).
    def __called(name)
      @_called_action = name if @_called_action.nil?
      nil
    end

    # Starts the action's chain, or answers false while another action
    # runs: a public method called from inside one is a plain call, as in
    # Rails, where only processing an action runs the callbacks.
    #
    # Only a mailer the class side or `with` built processes an action, as
    # in Rails: `Mailer.new.action` is a plain call too.
    def __start_action(name)
      return false unless @_processing && @_action_name.nil?
      @_action_name = name
      @_message = nil
      @_headers = {}
      true
    end

    # Ends the chain `__start_action` started.
    def __end_action
      @_action_name = nil
      nil
    end

    # `headers("X-Priority" => "1")` adds them; `headers["X"] = value`
    # adds one through the answer.
    def headers(values = nil)
      values.each { |name, value| __add_header(name, __header_text(name, value)) } unless values.nil?
      Headers.new(self)
    end

    # A list value as the mail gem writes it: a message-id list
    # (`References`, `In-Reply-To`) folds one id per line, any other
    # field joins with ", " (lobsters' reply mail sets `References` to the
    # thread's ids).
    def __header_text(name, value)
      return value.to_s unless value.is_a?(Array)
      parts = []
      value.each { |part| parts << part.to_s }
      field = name.downcase
      separator = field == "references" || field == "in-reply-to" ? "\r\n " : ", "
      parts.join(separator)
    end

    def __headers
      @_headers ||= {}
    end

    def __add_header(name, value)
      __headers[name] = value
      built = @_message
      built.add_header(name, value) unless built.nil?
      value
    end

    def params
      @params ||= {}
    end

    def _with_params(params)
      @params = params
      __processing
    end

    # Marks a mailer the class side (`Mailer.action(...)`) or `with` built:
    # its actions run their chains.
    def __processing
      @_processing = true
      self
    end
  end

  # What `headers` answers: `headers["X"] = value` adds a header to the
  # mail, or to the one `mail` will build.
  class Headers
    def initialize(mailer)
      @mailer = mailer
    end

    def []=(name, value)
      @mailer.__add_header(name, value)
    end

    def [](name)
      @mailer.__headers[name]
    end
  end
end
