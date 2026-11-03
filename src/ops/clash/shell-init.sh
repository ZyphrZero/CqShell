clashctl() {
    local _cqs_proxy_script
    case "${1-}" in
        on)
            if [ "$#" -eq 1 ]; then
                _cqs_proxy_script="$(command __CQS_EXECUTABLE__ proxy-env on --shell __CQS_SHELL__)" || return $?
                eval "$_cqs_proxy_script"
                return $?
            fi
            ;;
        off)
            if [ "$#" -eq 1 ] || { [ "$#" -eq 2 ] && [ "$2" = --stop ]; }; then
                __CQS_CLEAR_PROXY__ || return $?
                if [ "$#" -eq 2 ]; then
                    command __CQS_EXECUTABLE__ clashctl stop
                    return $?
                fi
                return 0
            fi
            ;;
    esac
    command __CQS_EXECUTABLE__ clashctl "$@"
}
