  root "articles#index"
  get "/literal", as: :literal, to: "defaults#show", defaults: { text: "fixed", symbol: :staff, number: 1, yes: true, no: false, absent: nil }
  get "/optional(/:number)", as: :optional, to: "defaults#show", defaults: { number: 1 }
  get "/number/:number", to: "defaults#show", as: :number, defaults: { number: 1 }
  match "/matched", to: "defaults#show", via: [:get, :post], defaults: { text: "fixed" }
  get "/duplicate", to: "defaults#show", defaults: { text: "first", text: "last" }
  get "/escaped", to: "defaults#show", defaults: { text: "a%20b+%2520" }
  get "/links", to: "defaults#links"
  get "/routing", to: "defaults#routing", defaults: { text: "fixed", symbol: :staff, number: 1, yes: true, no: false, absent: nil }
  get "/byte", to: "defaults#show", defaults: { text: "%FF" }
  get "/entry/:id(/page/:number)", as: :entry, to: "defaults#show", defaults: { number: 1 }
  get "(/:locale)/catalog", as: :catalog, to: "defaults#show", defaults: { locale: "en" }
  get "/archive(/:year)", as: :archive, to: "defaults#archive", defaults: { year: 2024 }
  get "/format_links", to: "defaults#format_links"
  get "/required/:number", to: "defaults#required", defaults: { number: 1 }
  get "/json_archive(/:year)", to: "defaults#json_archive", defaults: { year: 2024, format: "json" }
