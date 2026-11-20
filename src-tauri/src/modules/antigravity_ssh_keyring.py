"""Install and read back the Linux Antigravity credential over Secret Service."""
import os
import sys

import dbus


BUS_NAME = "org.freedesktop.secrets"
PREFIX = "org.freedesktop.Secret."


def sync_credential(payload):
    bus = dbus.bus.BusConnection(os.environ["DBUS_SESSION_BUS_ADDRESS"])

    def interface(path, name):
        return dbus.Interface(bus.get_object(BUS_NAME, path), PREFIX + name)

    service = interface("/org/freedesktop/secrets", "Service")
    collection_path = service.ReadAlias("default")
    if collection_path == "/":
        raise RuntimeError("No default keyring. Open the remote desktop keyring and set a default.")
    collection = interface(collection_path, "Collection")
    attributes = dbus.Dictionary(
        {"service": "gemini", "username": "antigravity"}, signature="ss"
    )
    paths = collection.SearchItems(attributes)
    session = service.OpenSession("plain", dbus.String("", variant_level=1))[1]
    try:
        secret = dbus.Struct(
            (session, dbus.ByteArray(b""), dbus.ByteArray(payload), "text/plain; charset=utf-8"),
            signature="oayays",
        )
        if paths:
            for path in paths:
                interface(path, "Item").SetSecret(secret)
        else:
            properties = dbus.Dictionary(
                {PREFIX + "Item.Label": "gemini", PREFIX + "Item.Attributes": attributes},
                signature="sv",
            )
            path, prompt = collection.CreateItem(properties, secret, True)
            if prompt != "/" or path == "/":
                raise RuntimeError("Unlock the remote desktop keyring before syncing.")
            paths = [path]
        for path in paths:
            stored = interface(path, "Item").GetSecret(session)
            if bytes(stored[2]) != payload:
                raise RuntimeError("Keyring read-back did not match the synced credential.")
    finally:
        interface(session, "Session").Close()


if __name__ == "__main__":
    try:
        sync_credential(sys.stdin.buffer.read())
    except dbus.DBusException as error:
        # D-Bus error messages can contain method arguments. Report only the error name.
        print(f"Linux keyring sync failed ({error.get_dbus_name()}). Check the remote desktop keyring.", file=sys.stderr)
        sys.exit(1)
    except RuntimeError as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
