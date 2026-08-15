function __cmd_x_main$main(){
  core_host$HostStdout_println([[],[]][1],[__cmd_x_main$roman(1),' ',__cmd_x_main$roman(9),' ',__cmd_x_main$roman(10),' ',__cmd_x_main$roman(11)]);
  return [0,0];
}
function __cmd_x_main$roman(n_0){
  return n_0===1?'I':n_0===2?'II':n_0===3?'III':n_0===4?'IV':n_0===5?'V':n_0===6?'VI':n_0===7?'VII':n_0===8?'VIII':n_0===9?'IX':n_0===10?'X':'?';
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
