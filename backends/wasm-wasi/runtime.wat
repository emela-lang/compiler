  ;; Indented 2 spaces: this fragment is embedded directly into a
  ;; `(module ...)` body, so it must match the surrounding indentation.

  (func $emela_strlen (param $ptr i32) (result i32)
    (local $end i32)
    local.get $ptr
    local.set $end
    (block $done
      (loop $scan
        local.get $end
        i32.load8_u
        i32.eqz
        br_if $done
        local.get $end
        i32.const 1
        i32.add
        local.set $end
        br $scan
      )
    )
    local.get $end
    local.get $ptr
    i32.sub
  )
  
  (func $emela_write_stdout_utf8 (param $str_ptr i32) (result i64)
    (local $len i32) (local $errno i32)
    local.get $str_ptr
    call $emela_strlen
    local.set $len
    i32.const 0x1000
    local.get $str_ptr
    i32.store
    i32.const 0x1004
    local.get $len
    i32.store
    i32.const 1
    i32.const 0x1000
    i32.const 1
    i32.const 0x1008
    call $wasi_fd_write
    local.set $errno
    (if (result i64) (i32.eqz (local.get $errno))
      (then (i64.const 0))
      (else
        local.get $errno
        i64.extend_i32_u
        i64.const 32
        i64.shl
        i64.const 1
        i64.or
      )
    )
  )
  
  (func $emela_write_stderr_utf8 (param $str_ptr i32) (result i64)
    (local $len i32) (local $errno i32)
    local.get $str_ptr
    call $emela_strlen
    local.set $len
    i32.const 0x1000
    local.get $str_ptr
    i32.store
    i32.const 0x1004
    local.get $len
    i32.store
    i32.const 2
    i32.const 0x1000
    i32.const 1
    i32.const 0x1008
    call $wasi_fd_write
    local.set $errno
    (if (result i64) (i32.eqz (local.get $errno))
      (then (i64.const 0))
      (else
        local.get $errno
        i64.extend_i32_u
        i64.const 32
        i64.shl
        i64.const 1
        i64.or
      )
    )
  )
  
  (func $emela_read_stdin_utf8 (result i64)
    (local $errno i32) (local $nread i32)
    i32.const 0x1000
    i32.const 0x2000
    i32.store
    i32.const 0x1004
    i32.const 4096
    i32.store
    i32.const 0x1008
    i32.const 0
    i32.store
    i32.const 0
    i32.const 0x1000
    i32.const 1
    i32.const 0x1008
    call $wasi_fd_read
    local.set $errno
    (if (result i64) (i32.eqz (local.get $errno))
      (then
        i32.const 0x1008
        i32.load
        local.set $nread
        i32.const 0x2000
        local.get $nread
        i32.add
        i32.const 0
        i32.store8
        i32.const 0x2000
        i64.extend_i32_u
        i64.const 32
        i64.shl
        i64.const 0
        i64.or
      )
      (else
        local.get $errno
        i64.extend_i32_u
        i64.const 32
        i64.shl
        i64.const 1
        i64.or
      )
    )
  )
  
  (func $emela_now_i32 (result i32)
    i32.const 0x1000
    i64.const 0
    i64.store
    i32.const 0
    i64.const 1
    i32.const 0x1000
    call $wasi_clock_time_get
    drop
    i32.const 0x1000
    i64.load
    i64.const 1000000
    i64.div_u
    i32.wrap_i64
  )
  
  (func $emela_random_i32 (result i32)
    i32.const 0x1000
    i32.const 4
    call $wasi_random_get
    drop
    i32.const 0x1000
    i32.load
  )
  
  (func $emela_read_file_utf8 (param $path_ptr i32) (result i64)
    (local $path_len i32) (local $fd i32) (local $errno i32) (local $nread i32)
    local.get $path_ptr
    call $emela_strlen
    local.set $path_len
    i32.const 0x1010
    i32.const -1
    i32.store
    i32.const 3
    i32.const 0
    local.get $path_ptr
    local.get $path_len
    i32.const 0
    i64.const 1
    i64.const 0
    i32.const 0
    i32.const 0x1010
    call $wasi_path_open
    local.set $errno
    (if (result i64) (i32.eqz (local.get $errno))
      (then
        i32.const 0x1010
        i32.load
        local.set $fd
        i32.const 0x1000
        i32.const 0x2000
        i32.store
        i32.const 0x1004
        i32.const 4096
        i32.store
        i32.const 0x1008
        i32.const 0
        i32.store
        local.get $fd
        i32.const 0x1000
        i32.const 1
        i32.const 0x1008
        call $wasi_fd_read
        drop
        local.get $fd
        call $wasi_fd_close
        drop
        i32.const 0x1008
        i32.load
        local.set $nread
        i32.const 0x2000
        local.get $nread
        i32.add
        i32.const 0
        i32.store8
        i32.const 0x2000
        i64.extend_i32_u
        i64.const 32
        i64.shl
        i64.const 0
        i64.or
      )
      (else
        local.get $errno
        i64.extend_i32_u
        i64.const 32
        i64.shl
        i64.const 1
        i64.or
      )
    )
  )
  
  (func $emela_write_file_utf8 (param $path_ptr i32) (param $data_ptr i32) (result i64)
    (local $path_len i32) (local $data_len i32) (local $fd i32) (local $errno i32)
    local.get $path_ptr
    call $emela_strlen
    local.set $path_len
    local.get $data_ptr
    call $emela_strlen
    local.set $data_len
    i32.const 0x1010
    i32.const -1
    i32.store
    i32.const 3
    i32.const 0
    local.get $path_ptr
    local.get $path_len
    i32.const 0
    i64.const 2
    i64.const 0
    i32.const 0
    i32.const 0x1010
    call $wasi_path_open
    local.set $errno
    (if (result i64) (i32.eqz (local.get $errno))
      (then
        i32.const 0x1010
        i32.load
        local.set $fd
        i32.const 0x1000
        local.get $data_ptr
        i32.store
        i32.const 0x1004
        local.get $data_len
        i32.store
        i32.const 0x1008
        i32.const 0
        i32.store
        local.get $fd
        i32.const 0x1000
        i32.const 1
        i32.const 0x1008
        call $wasi_fd_write
        drop
        local.get $fd
        call $wasi_fd_close
        drop
        i64.const 0
      )
      (else
        local.get $errno
        i64.extend_i32_u
        i64.const 32
        i64.shl
        i64.const 1
        i64.or
      )
    )
  )
  
  (func $emela_get_env (param $key_ptr i32) (result i64)
    (local $key_len i32) (local $count i32) (local $i i32) (local $env_ptr i32)
    (local $k i32) (local $ch_a i32) (local $ch_b i32)
    local.get $key_ptr
    call $emela_strlen
    local.set $key_len
    i32.const 0x1000
    i32.const 0
    i32.store
    i32.const 0x1004
    i32.const 0
    i32.store
    i32.const 0x1000
    i32.const 0x1004
    call $wasi_environ_sizes_get
    drop
    i32.const 0x1000
    i32.load
    local.set $count
    i32.const 0x2000
    i32.const 0x3000
    call $wasi_environ_get
    drop
    i32.const 0
    local.set $i
    (block $not_found
      (loop $search
        local.get $i
        local.get $count
        i32.ge_u
        br_if $not_found
        i32.const 0x2000
        local.get $i
        i32.const 4
        i32.mul
        i32.add
        i32.load
        local.set $env_ptr
        i32.const 0
        local.set $k
        (block $cmp_end
          (loop $cmp
            local.get $k
            local.get $key_len
            i32.ge_u
            br_if $cmp_end
            local.get $env_ptr
            local.get $k
            i32.add
            i32.load8_u
            local.set $ch_a
            local.get $key_ptr
            local.get $k
            i32.add
            i32.load8_u
            local.set $ch_b
            local.get $ch_a
            local.get $ch_b
            i32.ne
            br_if $cmp_end
            local.get $k
            i32.const 1
            i32.add
            local.set $k
            br $cmp
          )
        )
        local.get $k
        local.get $key_len
        i32.ge_u
        (if
          (then
            local.get $env_ptr
            local.get $key_len
            i32.add
            i32.load8_u
            i32.const 61
            i32.eq
            (if
              (then
                local.get $env_ptr
                local.get $key_len
                i32.add
                i32.const 1
                i32.add
                i64.extend_i32_u
                i64.const 32
                i64.shl
                i64.const 0
                i64.or
                return
              )
            )
          )
        )
        local.get $i
        i32.const 1
        i32.add
        local.set $i
        br $search
      )
    )
    i64.const 1
  )
