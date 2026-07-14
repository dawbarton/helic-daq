"""Wire-level codecs for HELIC-DAQ protocol v2.

Everything is little-endian. The authoritative description is
`docs/protocol.md`; its known-answer vectors are tested against this module
alongside the Rust and Python implementations.
"""
module Protocol

export BEACON_REQUEST,
    CONTROL_PORT,
    DISCOVERY_PORT,
    ERROR_BUSY,
    ERROR_NAMES,
    HEADER_LEN,
    MAGIC,
    MAX_PAYLOAD,
    STREAM_HEADER_LEN,
    STREAM_PORT,
    TRAILER_LEN,
    VERSION,
    BeaconResponse,
    MessageType,
    ProtocolError,
    StreamHeader,
    decode_beacon_response,
    decode_frame,
    decode_params,
    decode_sources,
    decode_stream_header,
    encode_beacon_response,
    encode_commit,
    encode_frame,
    encode_set_block,
    encode_stream_header,
    crc16

const MAGIC = UInt16(0x4c48)  # little-endian ASCII "HL"
const VERSION = UInt8(2)
const CONTROL_PORT = 2350
const STREAM_PORT = 2351
const DISCOVERY_PORT = 2352
const HEADER_LEN = 6
const TRAILER_LEN = 2
const MAX_PAYLOAD = 1024
const STREAM_HEADER_LEN = 20
# Error code (payload byte 1 of an Error response) the device returns while a
# table commit is still pending; hosts may retry.
const ERROR_BUSY = UInt8(7)

const ERROR_NAMES = Dict{UInt8, String}(
    1 => "bad frame",
    2 => "unknown message type",
    3 => "bad parameter index",
    4 => "bad length",
    5 => "parameter is read-only",
    6 => "bad value",
    7 => "device busy",
)

@enum MessageType::UInt8 begin
    GET_PARAMS = 1
    GET_SOURCES = 2
    GET_PAR = 3
    SET_PAR = 4
    SET_BLOCK = 5
    COMMIT = 6
    STREAM_SETUP = 7
    STREAM_START = 8
    STREAM_STOP = 9
    STATUS = 10
    ERROR = 0xff
end

struct ProtocolError <: Exception
    message::String
end

Base.showerror(io::IO, error::ProtocolError) = print(io, error.message)

struct BeaconResponse
    version::UInt8
    control_port::UInt16
    mac::NTuple{6, UInt8}
    experiment::String
    firmware::String
end

struct StreamHeader
    n_sources::UInt8
    seq::UInt32
    first_index::UInt32
    dropped::UInt32
    decimation::UInt16
    n_records::UInt16
end

_write_le(io::IO, value::UInt8) = write(io, value)
_write_le(io::IO, value::Int8) = write(io, value)
_write_le(io::IO, value::UInt16) = write(io, htol(value))
_write_le(io::IO, value::Int16) = write(io, htol(value))
_write_le(io::IO, value::UInt32) = write(io, htol(value))
_write_le(io::IO, value::Int32) = write(io, htol(value))
_write_le(io::IO, value::Float32) = _write_le(io, reinterpret(UInt32, value))

_read_le(io::IO, ::Type{UInt8}) = read(io, UInt8)
_read_le(io::IO, ::Type{Int8}) = read(io, Int8)
_read_le(io::IO, ::Type{UInt16}) = ltoh(read(io, UInt16))
_read_le(io::IO, ::Type{Int16}) = ltoh(read(io, Int16))
_read_le(io::IO, ::Type{UInt32}) = ltoh(read(io, UInt32))
_read_le(io::IO, ::Type{Int32}) = ltoh(read(io, Int32))
_read_le(io::IO, ::Type{Float32}) = reinterpret(Float32, _read_le(io, UInt32))

"""CRC-16/CCITT-FALSE (polynomial 0x1021, initial value 0xffff)."""
function crc16(data)::UInt16
    crc = UInt16(0xffff)
    for byte in data
        crc ⊻= UInt16(byte) << 8
        for _ in 1:8
            crc = (crc & 0x8000) != 0 ? (crc << 1) ⊻ 0x1021 : crc << 1
        end
    end
    return crc
end

const BEACON_REQUEST = let io = IOBuffer()
    _write_le(io, MAGIC)
    _write_le(io, UInt8(1))
    take!(io)
end

function encode_frame(message_type, sequence::Integer, payload = UInt8[])
    length(payload) <= MAX_PAYLOAD ||
        throw(ProtocolError("payload too long ($(length(payload)) > $MAX_PAYLOAD)"))
    body = IOBuffer()
    _write_le(body, UInt8(message_type))
    _write_le(body, sequence % UInt8)  # truncating conversion, like Python's & 0xff
    _write_le(body, UInt16(length(payload)))
    write(body, payload)
    body_bytes = take!(body)

    frame = IOBuffer()
    _write_le(frame, MAGIC)
    write(frame, body_bytes)
    _write_le(frame, crc16(body_bytes))
    return take!(frame)
end

function decode_frame(frame::AbstractVector{UInt8})
    length(frame) >= HEADER_LEN + TRAILER_LEN || throw(ProtocolError("frame truncated"))
    io = IOBuffer(frame)
    magic = _read_le(io, UInt16)
    magic == MAGIC || throw(ProtocolError("bad frame magic"))
    message_type = _read_le(io, UInt8)
    sequence = _read_le(io, UInt8)
    payload_length = Int(_read_le(io, UInt16))
    length(frame) == HEADER_LEN + payload_length + TRAILER_LEN ||
        throw(ProtocolError("frame length mismatch"))
    payload = read(io, payload_length)
    stored_crc = _read_le(io, UInt16)
    crc16(@view frame[3:(HEADER_LEN + payload_length)]) == stored_crc ||
        throw(ProtocolError("CRC mismatch"))
    return (message_type = message_type, sequence = sequence, payload = payload)
end

function _nul_string(payload::AbstractVector{UInt8}, offset::Int)
    ending = findnext(==(0x00), payload, offset)
    isnothing(ending) && throw(ProtocolError("unterminated discovery string"))
    bytes = @view payload[offset:(ending - 1)]
    all(<(0x80), bytes) || throw(ProtocolError("non-ASCII discovery string"))
    return String(bytes), ending + 1
end

function decode_params(payload::AbstractVector{UInt8})
    Definition = NamedTuple{
        (:name, :type_code, :count, :writable),
        Tuple{String, Char, UInt16, Bool},
    }
    definitions = Definition[]
    offset = 1
    while offset <= length(payload)
        name, offset = _nul_string(payload, offset)
        offset + 3 <= length(payload) ||
            throw(ProtocolError("truncated parameter definition"))
        type_code = Char(payload[offset])
        type_code in "BbHhIifc" ||
            throw(ProtocolError("invalid parameter type code '$type_code'"))
        count = UInt16(payload[offset + 1]) | (UInt16(payload[offset + 2]) << 8)
        writable_byte = payload[offset + 3]
        writable_byte <= 1 || throw(ProtocolError("invalid writable flag"))
        push!(definitions, (; name, type_code, count, writable = Bool(writable_byte)))
        offset += 4
    end
    return definitions
end

function decode_sources(payload::AbstractVector{UInt8})
    definitions = NamedTuple{(:name, :unit), Tuple{String, String}}[]
    offset = 1
    while offset <= length(payload)
        name, offset = _nul_string(payload, offset)
        unit, offset = _nul_string(payload, offset)
        push!(definitions, (; name, unit))
    end
    return definitions
end

function encode_set_block(index::Integer, offset::Integer, data)
    io = IOBuffer()
    _write_le(io, UInt16(index))
    _write_le(io, UInt32(offset))
    write(io, data)
    return take!(io)
end

function encode_commit(index::Integer, length::Integer)
    io = IOBuffer()
    _write_le(io, UInt16(index))
    _write_le(io, UInt32(length))
    return take!(io)
end

function _fixed_ascii(value::AbstractString)
    isascii(value) || throw(ArgumentError("beacon identities must be ASCII"))
    bytes = collect(codeunits(value))
    resize!(bytes, min(length(bytes), 16))
    append!(bytes, zeros(UInt8, 16 - length(bytes)))
    return bytes
end

function encode_beacon_response(response::BeaconResponse)
    io = IOBuffer()
    _write_le(io, MAGIC)
    _write_le(io, UInt8(2))
    _write_le(io, response.version)
    _write_le(io, response.control_port)
    write(io, collect(response.mac))
    write(io, _fixed_ascii(response.experiment))
    write(io, _fixed_ascii(response.firmware))
    return take!(io)
end

function decode_beacon_response(payload::AbstractVector{UInt8})
    length(payload) == 44 || throw(ProtocolError("bad beacon response length"))
    io = IOBuffer(payload)
    magic = _read_le(io, UInt16)
    kind = _read_le(io, UInt8)
    magic == MAGIC && kind == 2 || throw(ProtocolError("bad beacon response magic/type"))
    version = _read_le(io, UInt8)
    control_port = _read_le(io, UInt16)
    mac = Tuple(read(io, 6))
    fixed_string(bytes) = begin
        ending = something(findfirst(==(0x00), bytes), length(bytes) + 1)
        text = @view bytes[1:(ending - 1)]
        all(<(0x80), text) || throw(ProtocolError("non-ASCII beacon identity"))
        String(text)
    end
    experiment = fixed_string(read(io, 16))
    firmware = fixed_string(read(io, 16))
    return BeaconResponse(version, control_port, mac, experiment, firmware)
end

function encode_stream_header(header::StreamHeader)
    io = IOBuffer()
    _write_le(io, MAGIC)
    _write_le(io, VERSION)
    _write_le(io, header.n_sources)
    _write_le(io, header.seq)
    _write_le(io, header.first_index)
    _write_le(io, header.dropped)
    _write_le(io, header.decimation)
    _write_le(io, header.n_records)
    return take!(io)
end

function decode_stream_header(payload::AbstractVector{UInt8})
    length(payload) >= STREAM_HEADER_LEN || throw(ProtocolError("stream packet too short"))
    io = IOBuffer(payload)
    magic = _read_le(io, UInt16)
    version = _read_le(io, UInt8)
    magic == MAGIC && version == VERSION ||
        throw(ProtocolError("bad stream packet magic/version"))
    return StreamHeader(
        _read_le(io, UInt8),
        _read_le(io, UInt32),
        _read_le(io, UInt32),
        _read_le(io, UInt32),
        _read_le(io, UInt16),
        _read_le(io, UInt16),
    )
end

end
