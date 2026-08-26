#!/usr/bin/env python3
"""Lector de referencia del formato .forge — solo biblioteca estándar.

No es parte de FORGE: es la prueba de que el formato es abierto de verdad.
Si este archivo deja de leer un `.forge`, o el formato cambió sin actualizar la
especificación, o la especificación es incorrecta. Las dos cosas son bugs.

    python3 docs/formato/lector_referencia.py documento.forge
"""
import io
import json
import struct
import sys
import zipfile

# --- CBOR mínimo: solo el subconjunto que FORGE emite -----------------------

BREAK = object()


class Cbor:
    def __init__(self, buf):
        self.b = buf
        self.i = 0

    def u8(self):
        v = self.b[self.i]
        self.i += 1
        return v

    def take(self, n):
        v = self.b[self.i:self.i + n]
        self.i += n
        return v

    def arg(self, ai):
        if ai < 24:
            return ai
        if ai == 24:
            return self.u8()
        if ai == 25:
            return struct.unpack(">H", self.take(2))[0]
        if ai == 26:
            return struct.unpack(">I", self.take(4))[0]
        if ai == 27:
            return struct.unpack(">Q", self.take(8))[0]
        if ai == 31:
            return None  # longitud indefinida
        raise ValueError(f"argumento CBOR no soportado: {ai}")

    def value(self):
        ib = self.u8()
        major, ai = ib >> 5, ib & 0x1F

        if major == 0:
            return self.arg(ai)
        if major == 1:
            return -1 - self.arg(ai)
        if major == 2:
            n = self.arg(ai)
            return self.take(n) if n is not None else self._indef(bytes)
        if major == 3:
            n = self.arg(ai)
            raw = self.take(n) if n is not None else self._indef(bytes)
            return raw.decode("utf-8")
        if major == 4:
            n = self.arg(ai)
            if n is None:
                out = []
                while (v := self.value()) is not BREAK:
                    out.append(v)
                return out
            return [self.value() for _ in range(n)]
        if major == 5:
            n = self.arg(ai)
            out = {}
            if n is None:
                while (k := self.value()) is not BREAK:
                    out[_key(k)] = self.value()
                return out
            for _ in range(n):
                k = self.value()
                out[_key(k)] = self.value()
            return out
        if major == 6:
            self.arg(ai)      # etiqueta: se ignora, se devuelve el contenido
            return self.value()
        if major == 7:
            if ai == 20:
                return False
            if ai == 21:
                return True
            if ai == 22:
                return None
            if ai == 25:
                return _f16(struct.unpack(">H", self.take(2))[0])
            if ai == 26:
                return struct.unpack(">f", self.take(4))[0]
            if ai == 27:
                return struct.unpack(">d", self.take(8))[0]
            if ai == 31:
                return BREAK
        raise ValueError(f"tipo CBOR no soportado: major={major} ai={ai}")

    def _indef(self, kind):
        parts = []
        while (v := self.value()) is not BREAK:
            parts.append(v)
        return b"".join(parts)


def _key(k):
    return k if isinstance(k, (str, int)) else repr(k)


def _f16(h):
    exp, man = (h >> 10) & 0x1F, h & 0x3FF
    sign = -1 if h & 0x8000 else 1
    if exp == 0:
        return sign * man * 2.0 ** -24
    if exp == 31:
        return sign * (float("inf") if man == 0 else float("nan"))
    return sign * (man + 1024) * 2.0 ** (exp - 25)


def decode(buf):
    return Cbor(buf).value()


# --- Lectura del contenedor -------------------------------------------------

def leer(path):
    with zipfile.ZipFile(path) as z:
        nombres = z.namelist()

        manifest = json.loads(z.read("manifest.json"))
        if manifest.get("format") != "forge":
            raise SystemExit(f"{path}: no es un documento FORGE")
        if manifest.get("format_version", 0) > 1:
            raise SystemExit(
                f"{path}: version de formato {manifest['format_version']}, "
                f"este lector entiende hasta la 1"
            )

        doc = decode(z.read("document.cbor"))
        # document.cbor = { entities: [id...], stores: [{name, data}...] }
        entities = doc["entities"]
        stores = {}
        for s in doc["stores"]:
            stores[s["name"]] = decode(bytes(s["data"]))

        blobs = [n for n in nombres if n.startswith("blobs/")]
        return manifest, entities, stores, blobs


def main():
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    path = sys.argv[1]
    manifest, entities, stores, blobs = leer(path)

    print(f"{path}")
    print(f"  formato   {manifest['format']} v{manifest['format_version']}")
    print(f"  unidades  {manifest['units']}   eje vertical  {manifest['up_axis']}")
    print(f"  tolerancia {manifest['tolerance_confusion_mm']}")
    print(f"  generador {manifest['generator']}")
    print(f"  entidades {len(entities)}")
    print(f"  blobs     {len(blobs)}")
    print("  componentes:")
    for name, entries in sorted(stores.items()):
        print(f"    {name:<20} {len(entries):>5} entidades")
    # Ejemplo de lectura real: los nombres
    nombres = stores.get("forge.name")
    if nombres:
        print("  nombres:")
        for _id, valor in nombres[:10]:
            print(f"    {valor}")


if __name__ == "__main__":
    main()
