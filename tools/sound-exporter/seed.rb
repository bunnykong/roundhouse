# Seed the real runtime app; tracing is disabled during dependency preparation.
module SoundRead
  def self.capture(value, *) = value
end
require "bundler/setup"
require "rake"
load File.join(ENV.fetch("SOUND_APP"), "Rakefile")
Rake::Task["db:seed"].invoke
