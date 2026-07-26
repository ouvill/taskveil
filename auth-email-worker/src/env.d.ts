interface CloudflareBindings {
  EMAIL_COMMANDS: Queue<EmailDeliveryCommand>;
  EMAIL_DELIVERY_LEDGER: DurableObjectNamespace;
  SEND_EMAIL: SendEmail;
  EMAIL_FROM: string;
  DELIVERY_SIGNING_KEY_CURRENT_ID: string;
  DELIVERY_SIGNING_KEY_CURRENT: string;
  DELIVERY_SIGNING_KEY_PREVIOUS_ID: string;
  DELIVERY_SIGNING_KEY_PREVIOUS: string;
  DATA_KEY_CURRENT_VERSION: string;
  DATA_KEY_CURRENT: string;
  DATA_KEY_PREVIOUS_VERSION: string;
  DATA_KEY_PREVIOUS: string;
  TEST_ONLY_DISABLE_LEDGER?: "enabled";
}

declare namespace Cloudflare {
  interface Env extends CloudflareBindings {}

  interface GlobalProps {
    durableNamespaces: "EmailDeliveryLedger";
    mainModule: typeof import("./index");
  }
}

interface EmailDeliveryCommand {
  version: number;
  delivery_id: string;
  not_after: string;
  encrypted_payload: string;
}
