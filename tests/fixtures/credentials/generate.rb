# Writes the sealed credentials of the request tests in tests/requests/credentials.rs.
# Every value is a public test value. The unlock key is bytes 0 to 31:
# AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=
require 'base64'
require 'fileutils'
require 'openssl'

unless RUBY_VERSION.split('.').first.to_i >= 3 && OpenSSL::OPENSSL_VERSION.start_with?('OpenSSL ')
  abort 'Use Ruby 3 or newer linked against OpenSSL to regenerate these fixtures.'
end

KEY = (0...32).to_a.pack('C*')
HEADER = "credseal-v1\n"

def seal(path, environment, nonce_byte, plaintext)
  application = 'keysend'
  aad = HEADER + [application.bytesize].pack('C') + application +
        [environment.bytesize].pack('C') + environment
  nonce = ([nonce_byte] * 12).pack('C*')
  cipher = OpenSSL::Cipher.new('aes-256-gcm')
  cipher.encrypt
  cipher.key = KEY
  cipher.iv = nonce
  cipher.auth_data = aad
  ciphertext = cipher.update(plaintext) + cipher.final
  target = File.join(__dir__, path, "#{environment}.toml.enc")
  FileUtils.mkdir_p(File.dirname(target))
  File.binwrite(target, HEADER + Base64.strict_encode64(nonce + ciphertext + cipher.auth_tag) + "\n")
end

seal('sealed', 'test', 1, <<~TOML)
  postdeploy_ingest_key = "sealed-postdeploy-secret"
  stripe_secret_key = "sk_test_sealed-stripe-secret"
  stripe_webhook_secret = "whsec_sealed-stripe-secret"
  openai_api_key = "sealed-openai-secret"
  cloudflare_email_token = "sealed-email-secret"
  report_email_recipient = "sealed@example.com"
  google_client_id = "sealed-google-client-id"
  google_client_secret = "sealed-google-client-secret"
  keysend_master_key = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="
  kamal_registry_password = "sealed-registry-secret"
  cloudflare_cache_token = "sealed-cache-secret"
  r2_access_key_id = "sealed-r2-access"
  r2_secret_access_key = "sealed-r2-secret"
  r2_endpoint = "https://example.r2.cloudflarestorage.com"
  cloudflare_origin_certificate = "sealed-certificate"
  cloudflare_origin_key = "sealed-origin-secret"
  ci_ssh_key = "sealed-ssh-secret"
  ci_known_hosts = "sealed-known-hosts"
  age_recipient = "sealed-age-recipient"
TOML

seal('unknown-field', 'test', 2, <<~TOML)
  google_client_id = "sealed-google-client-id"
  google_client_scret = "misspelled-google-client-secret"
TOML

seal('without-data-key', 'production', 3, <<~TOML)
  google_client_id = "sealed-google-client-id"
  google_client_secret = "sealed-google-client-secret"
TOML

seal('backup-only', 'test', 4, <<~TOML)
  r2_access_key_id = "sealed-r2-access"
  r2_secret_access_key = "sealed-r2-secret"
  r2_endpoint = "https://example.r2.cloudflarestorage.com"
  age_recipient = "sealed-age-recipient"
TOML

seal('nul-byte', 'test', 5, <<~'TOML')
  r2_access_key_id = "sealed-r2-access"
  r2_secret_access_key = "sealed-r2\u0000secret"
  r2_endpoint = "https://example.r2.cloudflarestorage.com"
  age_recipient = "sealed-age-recipient"
TOML
