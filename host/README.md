# cbc-daq (host package)

Python interface to the CBC-DAQ device: parameter get/set over TCP,
sample streaming over UDP, and a `cbc-daq` command-line tool.

Install (from the repository root):

```sh
pip install -e host          # add [plot] for the plotting extra
```

Quick start:

```python
from cbc_daq import Device, StreamReceiver

dev = Device("192.168.1.235")
print(dev.params)                 # discovered parameter list
dev.par.freq = 10.0               # attribute-style access
data = dev.capture(["adc0", "out"], seconds=2.0)
```

See `docs/user_guide.md` in the repository for the full guide and
`docs/protocol.md` for the wire protocol.
