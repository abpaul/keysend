require 'base64'
require 'openssl'

unless RUBY_VERSION.split('.').first.to_i >= 3 && OpenSSL::OPENSSL_VERSION.start_with?('OpenSSL ')
  abort 'Use Ruby 3 or newer linked against OpenSSL to regenerate this fixture.'
end

key = (0...32).to_a.pack('C*')
nonce = (100...112).to_a.pack('C*')
application = 'keysend'
environment = 'production'
header = "credseal-v1\n"
aad = header + [application.bytesize].pack('C') + application +
      [environment.bytesize].pack('C') + environment
plaintext = File.binread(File.join(__dir__, 'credentials.toml'))

cipher = OpenSSL::Cipher.new('aes-256-gcm')
cipher.encrypt
cipher.key = key
cipher.iv = nonce
cipher.auth_data = aad
ciphertext = cipher.update(plaintext) + cipher.final
file = header + Base64.strict_encode64(nonce + ciphertext + cipher.auth_tag) + "\n"
File.binwrite(File.join(__dir__, 'production.toml.enc'), file)
