#!/usr/bin/env python3
"""Creates a shadNet account: command Create (2) with a RegistrationRequest protobuf payload, as in
shadNet's documentation/protocol.md. An admin tool: players register through the backend
(POST /account/v1/register).

  python create_account.py <host> <port> <online_id> <email>

The password comes from FZH_ACCOUNT_PASSWORD, else it is asked for without echo. The registration
key comes from FZH_REGISTRATION_KEY, else it is asked for (empty = the server has no key).
--dry-run prints the request packet in hex instead of sending it.
"""
import argparse, getpass, os, sys

# Installed copies sit in one folder with shadnet_client.py; in the repository it is in
# server/backend.
sys.path.append(os.path.join(os.path.dirname(os.path.abspath(__file__)), os.pardir, "backend"))
import shadnet_client  # noqa: E402

RESULTS = {shadnet_client.NO_ERROR: "NoError (account created)",
           shadnet_client.CREATION_EXISTING_USERNAME: "error 11: Online ID already registered",
           shadnet_client.CREATION_EXISTING_EMAIL: "error 13: email already registered",
           shadnet_client.UNAUTHORIZED: "error 23: wrong or missing registration key"}


def main(argv):
    p = argparse.ArgumentParser(description="Create a shadNet account")
    p.add_argument("host")
    p.add_argument("port", type=int)
    p.add_argument("online_id", help="3-16 characters: letters, digits, _ and -")
    p.add_argument("email", help="must be unique per account; no mail is sent")
    p.add_argument("--avatar-url", default=None,
                   help="default http://fzh.avatar/a/<online_id>.png, served by the backend")
    p.add_argument("--dry-run", action="store_true", help="print the packet in hex, do not connect")
    a = p.parse_args(argv)
    password = os.environ.get("FZH_ACCOUNT_PASSWORD") or getpass.getpass("Account password: ")
    key = os.environ.get("FZH_REGISTRATION_KEY")
    if key is None:
        key = getpass.getpass("Server registration key (empty if none): ")
    avatar_url = a.avatar_url if a.avatar_url is not None else "http://fzh.avatar/a/%s.png" % a.online_id
    if a.dry_run:
        payload = shadnet_client.registration(a.online_id, password, avatar_url, a.email, key)
        print(shadnet_client.packet(shadnet_client.CREATE, payload, 1).hex())
        return 0
    code = shadnet_client.create_account(a.host, a.port, a.online_id, password, avatar_url, a.email, key)
    print("server reply: %s" % RESULTS.get(code, "error %d (see protocol.md)" % code))
    return 0 if code == shadnet_client.NO_ERROR else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
