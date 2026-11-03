function clashctl
    if test (count $argv) -eq 1; and test "$argv[1]" = on
        set -l _cqs_proxy_script (command __CQS_EXECUTABLE__ proxy-env on --shell fish)
        set -l _cqs_proxy_status $status
        if test $_cqs_proxy_status -ne 0
            return $_cqs_proxy_status
        end
        printf '%s\n' $_cqs_proxy_script | source
        return $status
    end
    if test (count $argv) -ge 1; and test "$argv[1]" = off
        if test (count $argv) -eq 1; or begin; test (count $argv) -eq 2; and test "$argv[2]" = --stop; end
            __CQS_CLEAR_PROXY__
            if test (count $argv) -eq 2
                command __CQS_EXECUTABLE__ clashctl stop
                return $status
            end
            return 0
        end
    end
    command __CQS_EXECUTABLE__ clashctl $argv
end
