const $k0=[0,0];
function __cmd_x_main_buri$main(){
  const ctx_0=[[],[]];
  const text_2=String(__cmd_x_main_buri$sumTo(100n,0n))+' '+String(__cmd_x_main_buri$fib(30n,0n,1n))+' '+String(__cmd_x_main_buri$countDigits(12345n,0n));
  const self_3=$host_HostStdout_println(ctx_0[1],text_2);
  let $t1;
  if(self_3[0]===0){
    $t1=0;
  }else if(self_3[0]===1){
    $t1=0;
  }else{
    $abort('no arm matched');
  }
  const text_7=String(__cmd_x_main_buri$swapDown(1n,2n,3n))+' '+String(__cmd_x_main_buri$swapDown(1n,2n,4n));
  const self_8=$host_HostStdout_println(ctx_0[1],text_7);
  let $t3;
  if(self_8[0]===0){
    $t3=0;
  }else if(self_8[0]===1){
    $t3=0;
  }else{
    $abort('no arm matched');
  }
  return $k0;
}
function __cmd_x_main_buri$sumTo(n_0,acc_1){
  while(true){
    if(n_0===0n){
      return acc_1;
    }else{
      const $t1=n_0-1n;
      acc_1=acc_1+n_0;
      n_0=$t1;
      continue;
    }
  }
}
function __cmd_x_main_buri$fib(n_0,a_1,b_2){
  while(true){
    if(n_0===0n){
      return a_1;
    }else{
      n_0=n_0-1n;
      const $t1=b_2;
      b_2=a_1+b_2;
      a_1=$t1;
      continue;
    }
  }
}
function __cmd_x_main_buri$countDigits(n_0,acc_1){
  while(true){
    if(n_0<10n){
      return acc_1+1n;
    }else{
      n_0=n_0/10n;
      acc_1=acc_1+1n;
      continue;
    }
  }
}
function __cmd_x_main_buri$swapDown(a_0,b_1,fuel_2){
  while(true){
    if(fuel_2===0n){
      return a_0*10n+b_1;
    }else{
      const $t1=b_1;
      b_1=a_0;
      fuel_2=fuel_2-1n;
      a_0=$t1;
      continue;
    }
  }
}
